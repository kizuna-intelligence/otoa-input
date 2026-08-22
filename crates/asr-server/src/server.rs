use crate::{
    asr::{Asr, Transcriber},
    config::Config,
    session::{Session, SessionState},
};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::{borrow::Cow, sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{ErrorResponse, Request, Response},
        http::{Response as HttpResponse, StatusCode},
        protocol::{frame::coding::CloseCode, CloseFrame, Message},
    },
};

pub async fn run(config: Config) -> Result<()> {
    let asr_model_dir = config.asr_model_dir.clone();
    let asr_threads = config.asr_threads;
    let asr = tokio::task::spawn_blocking(move || Asr::load(&asr_model_dir, asr_threads))
        .await
        .context("ASR model worker failed")??;
    let asr = Arc::new(asr);

    let listener = TcpListener::bind((config.host.as_str(), config.port))
        .await
        .with_context(|| format!("failed to bind {}:{}", config.host, config.port))?;
    tracing::info!(
        host = %config.host,
        port = config.port,
        path = %config.path,
        pseudo_stream = config.pseudo_stream,
        "ASR server listening"
    );

    let config = Arc::new(config);
    loop {
        let (stream, peer) = listener
            .accept()
            .await
            .context("failed to accept connection")?;
        let config = Arc::clone(&config);
        let asr = Arc::clone(&asr);
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, peer, config, asr).await {
                tracing::error!(%error, "ASR connection failed");
            }
        });
    }
}

async fn handle_connection(
    stream: TcpStream,
    peer: std::net::SocketAddr,
    config: Arc<Config>,
    asr: Arc<Asr>,
) -> Result<()> {
    let callback_config = Arc::clone(&config);
    // Err の型は tungstenite のハンドシェイクコールバックが決めており、
    // こちらで箱に入れられない。Windows ではこの型が 136 バイトになり、
    // clippy の既定のしきい値 128 バイトを超える。
    #[allow(clippy::result_large_err)]
    let callback = move |request: &Request, response: Response| {
        if !authorization_is_valid(request, callback_config.auth_token.as_deref()) {
            return Err(http_rejection(StatusCode::UNAUTHORIZED, "unauthorized\n"));
        }
        if request.uri().path() != callback_config.path {
            return Err(http_rejection(StatusCode::NOT_FOUND, "not found\n"));
        }
        Ok(response)
    };

    let mut websocket = match accept_hdr_async(stream, callback).await {
        Ok(websocket) => websocket,
        Err(error) => {
            // Authentication and path failures have already been written as
            // HTTP responses by the handshake callback and are not websocket
            // session failures.
            tracing::debug!(peer = %peer, %error, "websocket handshake rejected");
            return Ok(());
        }
    };
    tracing::info!(peer = %peer, "connection accepted");

    let transcriber: Arc<dyn Transcriber> = asr;
    let mut session = Session::new((*config).clone(), transcriber);
    let mut close_code = None;

    while let Some(message) = websocket.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                tracing::error!(peer = %peer, %error, "websocket receive failed");
                break;
            }
        };
        let action = match message {
            Message::Text(text) => session.handle_text(&text).await,
            Message::Binary(bytes) => session.handle_binary(&bytes).await,
            Message::Ping(payload) => {
                websocket.send(Message::Pong(payload)).await?;
                continue;
            }
            Message::Pong(_) | Message::Frame(_) => continue,
            Message::Close(frame) => {
                close_code = frame.as_ref().map(|close| u16::from(close.code));
                if let Err(error) = websocket.send(Message::Close(frame)).await {
                    tracing::debug!(peer = %peer, %error, "failed to acknowledge websocket close");
                }
                break;
            }
        };

        match action {
            Ok(action) => {
                send_responses(&mut websocket, action.responses).await?;
                if let Some(code) = action.close_code {
                    close_code = Some(code);
                    close_and_wait(&mut websocket, code).await?;
                    break;
                }
            }
            Err(error) => {
                tracing::error!(peer = %peer, %error, "session error");
                send_responses(&mut websocket, vec![error.error_response()]).await?;
                let code = error.close_code();
                close_code = Some(code);
                close_and_wait(&mut websocket, code).await?;
                break;
            }
        }
    }

    session.shutdown().await;
    if close_code.is_none() && session.state == SessionState::Closed {
        close_code = Some(1006);
    }
    tracing::info!(peer = %peer, close_code = ?close_code, "connection disconnected");
    Ok(())
}

fn authorization_is_valid(request: &Request, expected_token: Option<&str>) -> bool {
    let Some(expected_token) = expected_token else {
        return true;
    };
    let Some(value) = request.headers().get("authorization") else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    constant_time_equal(
        value.as_bytes(),
        format!("Bearer {expected_token}").as_bytes(),
    )
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(
            left.get(index).copied().unwrap_or_default()
                ^ right.get(index).copied().unwrap_or_default(),
        );
    }
    difference == 0
}

fn http_rejection(status: StatusCode, body: &str) -> ErrorResponse {
    HttpResponse::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("Content-Length", body.len().to_string())
        .body(Some(body.to_string()))
        .expect("valid HTTP rejection response")
}

async fn send_responses(
    websocket: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
    responses: Vec<otoa_input_protocol::AsrResponse>,
) -> Result<()> {
    for response in responses {
        let text = serde_json::to_string(&response).context("failed to encode ASR response")?;
        websocket.send(Message::Text(text)).await?;
    }
    Ok(())
}

async fn close_and_wait(
    websocket: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
    code: u16,
) -> Result<()> {
    let close = Message::Close(Some(CloseFrame {
        code: CloseCode::from(code),
        reason: Cow::Borrowed(""),
    }));
    websocket.send(close).await?;

    let wait_for_ack = async {
        while let Some(message) = websocket.next().await {
            match message? {
                Message::Close(_) => break,
                Message::Ping(payload) => websocket.send(Message::Pong(payload)).await?,
                Message::Pong(_) | Message::Text(_) | Message::Binary(_) | Message::Frame(_) => {}
            }
        }
        Ok::<(), tokio_tungstenite::tungstenite::Error>(())
    };
    if timeout(Duration::from_secs(5), wait_for_ack).await.is_err() {
        tracing::warn!(
            code,
            "timed out waiting for websocket close acknowledgement"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::constant_time_equal;

    #[test]
    fn authorization_comparison_checks_length_and_content() {
        assert!(constant_time_equal(b"Bearer token", b"Bearer token"));
        assert!(!constant_time_equal(b"Bearer token", b"Bearer other"));
        assert!(!constant_time_equal(
            b"Bearer token",
            b"Bearer token-longer"
        ));
    }
}
