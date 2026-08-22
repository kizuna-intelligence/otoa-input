use crate::{protocol::parse_response, AsrConfig, AsrError, AsrToken};
use crossbeam_channel::{Receiver, TryRecvError};
use std::{
    io::ErrorKind,
    net::TcpStream,
    thread,
    time::{Duration, Instant},
};
use tungstenite::{connect, stream::MaybeTlsStream, ClientRequestBuilder, Message, WebSocket};

/// client → session へ送る指示。
pub enum AsrCommand {
    /// 生 PCM（16 kHz mono s16le のバイト列）。
    Audio(Vec<u8>),
    /// 手動確定を要求する。`{"type":"finalize"}` を送る。
    Finalize,
    /// 音声送信を終える。空フレームを送り finished を待つ。
    Stop,
}

/// session → client へ返す出来事。
#[derive(Debug)]
pub enum AsrEvent {
    /// 接続と config 送信が完了した。
    Connected,
    /// 確定トークン（`<end>` / `<fin>` を除く）。
    FinalText(Vec<AsrToken>),
    /// 未確定トークン。受信のたびに前回分を置き換える。
    PartialText(Vec<AsrToken>),
    /// `<end>` を受信した。発話の区切り。
    Endpoint,
    /// `<fin>` を受信した。手動確定の完了。
    FinalizeDone,
    /// `finished: true` を受信した。この後 close される。
    Finished,
    /// 復帰不能な失敗。この後スレッドは終了する。
    Failed(AsrError),
}

pub struct AsrSession;

impl AsrSession {
    /// 別スレッドで接続し、送受信ループを回す。
    pub fn spawn(
        url: String,
        config: AsrConfig,
        headers: Vec<(String, String)>,
        commands: Receiver<AsrCommand>,
        events: crossbeam_channel::Sender<AsrEvent>,
    ) -> thread::JoinHandle<()> {
        thread::Builder::new()
            .name("otoa-asr".to_string())
            .spawn(move || run_session(&url, config, headers, commands, events))
            .expect("failed to spawn ASR session thread")
    }
}

fn run_session(
    url: &str,
    config: AsrConfig,
    headers: Vec<(String, String)>,
    commands: Receiver<AsrCommand>,
    events: crossbeam_channel::Sender<AsrEvent>,
) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let uri = match url.parse() {
        Ok(uri) => uri,
        Err(error) => {
            let _ = events.send(AsrEvent::Failed(AsrError::Connect(format!(
                "invalid websocket URL: {error}"
            ))));
            return;
        }
    };
    let request = headers
        .into_iter()
        .fold(ClientRequestBuilder::new(uri), |request, (name, value)| {
            request.with_header(name, value)
        });
    let (mut ws, _) = match connect(request) {
        Ok(connection) => connection,
        Err(error) => {
            let _ = events.send(AsrEvent::Failed(AsrError::Connect(error.to_string())));
            return;
        }
    };

    set_read_timeout(&mut ws);

    let config_text = match serde_json::to_string(&config) {
        Ok(text) => text,
        Err(error) => {
            let _ = events.send(AsrEvent::Failed(AsrError::Io(format!(
                "serialize config: {error}"
            ))));
            return;
        }
    };
    if let Err(error) = send_message(&mut ws, Message::Text(config_text)) {
        let _ = events.send(AsrEvent::Failed(error));
        return;
    }

    let _ = events.send(AsrEvent::Connected);
    let mut last_sent = Instant::now();
    let mut stopping = false;
    let mut finished_received = false;
    let mut failed = false;

    'session: loop {
        for _ in 0..32 {
            match commands.try_recv() {
                Ok(AsrCommand::Audio(bytes)) => {
                    if let Err(error) = send_message(&mut ws, Message::Binary(bytes)) {
                        send_failed(&events, error);
                        failed = true;
                        break 'session;
                    }
                    last_sent = Instant::now();
                }
                Ok(AsrCommand::Finalize) => {
                    if let Err(error) =
                        send_message(&mut ws, Message::Text(r#"{"type":"finalize"}"#.to_string()))
                    {
                        send_failed(&events, error);
                        failed = true;
                        break 'session;
                    }
                    last_sent = Instant::now();
                }
                Ok(AsrCommand::Stop) => {
                    if let Err(error) = send_message(&mut ws, Message::Text(String::new())) {
                        send_failed(&events, error);
                        failed = true;
                        break 'session;
                    }
                    last_sent = Instant::now();
                    stopping = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break 'session,
            }
        }

        match ws.read() {
            Ok(Message::Text(text)) => {
                log_response_summary(text.as_ref());
                match parse_response(&text) {
                    Ok(response_events) => {
                        for event in response_events {
                            if matches!(event, AsrEvent::Finished) {
                                finished_received = true;
                            }
                            if matches!(event, AsrEvent::Failed(_)) {
                                failed = true;
                            }
                            if let AsrEvent::Failed(AsrError::Server { request_id, .. }) = &event {
                                tracing::error!(?request_id, "ASR server error");
                            }
                            let is_failed = matches!(event, AsrEvent::Failed(_));
                            let _ = events.send(event);
                            if is_failed {
                                break 'session;
                            }
                        }
                    }
                    Err(error) => {
                        send_failed(&events, error);
                        failed = true;
                        break 'session;
                    }
                }
            }
            Ok(Message::Close(_)) => {
                if !stopping {
                    send_failed(&events, AsrError::ClosedEarly);
                    failed = true;
                }
                break 'session;
            }
            Ok(Message::Binary(_))
            | Ok(Message::Ping(_))
            | Ok(Message::Pong(_))
            | Ok(Message::Frame(_)) => {}
            Err(error) if is_read_timeout(&error) => {}
            Err(error) if stopping || finished_received => {
                tracing::debug!(
                    target: "otoa_input",
                    "ASR connection closed during normal shutdown: {error}"
                );
                break 'session;
            }
            Err(error) => {
                send_failed(&events, AsrError::Io(error.to_string()));
                failed = true;
                break 'session;
            }
        }

        if last_sent.elapsed() >= Duration::from_secs(15) {
            if let Err(error) = send_message(
                &mut ws,
                Message::Text(r#"{"type":"keepalive"}"#.to_string()),
            ) {
                send_failed(&events, error);
                failed = true;
                break 'session;
            }
            last_sent = Instant::now();
        }

        if finished_received {
            let _ = ws.close(None);
            break 'session;
        }
    }

    if !failed && !finished_received {
        let _ = events.send(AsrEvent::Finished);
    }
}

fn log_response_summary(text: &str) {
    let Ok(response) = serde_json::from_str::<crate::AsrResponse>(text) else {
        tracing::debug!(
            target: "otoa_input",
            "tokens=0 final=0 nonfinal=0 has_end=false has_fin=false finished=false"
        );
        return;
    };

    let final_count = response
        .tokens
        .iter()
        .filter(|token| token.is_final)
        .count();
    let nonfinal_count = response.tokens.len().saturating_sub(final_count);
    let has_end = response.tokens.iter().any(|token| token.is_endpoint());
    let has_fin = response
        .tokens
        .iter()
        .any(|token| token.is_finalize_marker());
    tracing::debug!(
        target: "otoa_input",
        "tokens={} final={} nonfinal={} has_end={} has_fin={} finished={}",
        response.tokens.len(),
        final_count,
        nonfinal_count,
        has_end,
        has_fin,
        response.finished
    );
}

fn send_message(
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    message: Message,
) -> Result<(), AsrError> {
    ws.send(message)
        .map_err(|error| AsrError::Io(error.to_string()))
}

fn send_failed(events: &crossbeam_channel::Sender<AsrEvent>, error: AsrError) {
    if let AsrError::Server { request_id, .. } = &error {
        tracing::error!(?request_id, "ASR server error");
    }
    let _ = events.send(AsrEvent::Failed(error));
}

fn is_read_timeout(error: &tungstenite::Error) -> bool {
    matches!(
        error,
        tungstenite::Error::Io(io_error)
            if matches!(io_error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
    )
}

fn set_read_timeout(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>) {
    let timeout = Some(Duration::from_millis(20));
    let result = match ws.get_mut() {
        MaybeTlsStream::Plain(stream) => stream.set_read_timeout(timeout),
        MaybeTlsStream::Rustls(stream) => stream.get_mut().set_read_timeout(timeout),
        _ => Ok(()),
    };
    if let Err(error) = result {
        tracing::warn!("failed to set ASR read timeout: {error}");
    }
}
