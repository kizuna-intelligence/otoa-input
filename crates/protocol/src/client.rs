use crate::{protocol::parse_response, AsrConfig, AsrError, AsrToken};
use crossbeam_channel::{Receiver, TryRecvError};
use std::{
    io::ErrorKind,
    net::TcpStream,
    thread,
    time::{Duration, Instant},
};
use tungstenite::{connect, stream::MaybeTlsStream, ClientRequestBuilder, Message, WebSocket};

/// WebSocket の Policy Violation close code。
pub const POLICY_VIOLATION_CLOSE_CODE: u16 = 1008;

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
    /// WebSocket の close frame。理由は相手が送った本文をそのまま保持する。
    Closed { code: Option<u16>, reason: String },
    /// セッションを継続したまま利用者へ伝える通知。
    Notice { code: String, message: String },
    /// サーバーが名乗った、このセッションで使われている方法。
    ///
    /// **利用者には見せない。** 頼んだ方法と違わないかを確かめるためだけに使う。
    Backend(String),
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
    ) -> Result<thread::JoinHandle<()>, AsrError> {
        thread::Builder::new()
            .name("otoa-asr".to_string())
            .spawn(move || run_session(&url, config, headers, commands, events))
            .map_err(|error| AsrError::Io(format!("failed to spawn ASR session thread: {error}")))
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
                            match &event {
                                AsrEvent::Finished => finished_received = true,
                                AsrEvent::Backend(backend) => {
                                    tracing::debug!(%backend, "ASR backend announced");
                                }
                                AsrEvent::Notice { code, .. } => {
                                    // Notice は利用者向けの非致命イベントである。failed
                                    // にせず、この WebSocket をそのまま受信し続ける。
                                    tracing::debug!(notice_code = %code, "ASR notice received");
                                }
                                AsrEvent::Failed(AsrError::Server { request_id, .. }) => {
                                    failed = true;
                                    tracing::error!(?request_id, "ASR server error");
                                }
                                AsrEvent::Failed(_) => failed = true,
                                _ => {}
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
            Ok(Message::Close(frame)) => {
                let (code, reason) = frame.map_or((None, String::new()), |frame| {
                    (Some(u16::from(frame.code)), frame.reason.to_string())
                });
                let _ = events.send(AsrEvent::Closed { code, reason });
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
            Err(error) if finished_received => {
                tracing::debug!(
                    target: "otoa_input",
                    "ASR connection closed during normal shutdown: {error}"
                );
                break 'session;
            }
            Err(error) if stopping => {
                send_failed(&events, AsrError::Io(error.to_string()));
                failed = true;
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

#[cfg(test)]
mod tests {
    use super::{AsrCommand, AsrSession};
    use crate::{AsrConfig, AsrEvent};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};
    use tungstenite::{accept, Message};

    #[test]
    fn notice_does_not_end_the_session() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("test listener should have an address");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("client should connect");
            let mut websocket = accept(stream).expect("websocket handshake should succeed");
            assert!(matches!(
                websocket.read().expect("config should arrive"),
                Message::Text(_)
            ));
            websocket
                .send(Message::Text(
                    r#"{"notice_code":"gate_blocked","notice_message":"登録した声と一致しませんでした。"}"#.to_string(),
                ))
                .expect("notice should be sent");
            websocket
                .send(Message::Text(
                    r#"{"tokens":[{"text":"次の応答","is_final":false}]}"#.to_string(),
                ))
                .expect("post-notice response should be sent");

            loop {
                match websocket
                    .read()
                    .expect("client should keep the session open")
                {
                    Message::Text(text) if text.is_empty() => {
                        websocket
                            .send(Message::Text(r#"{"finished":true}"#.to_string()))
                            .expect("finished response should be sent");
                        return;
                    }
                    Message::Close(_) => return,
                    _ => {}
                }
            }
        });

        let (to_session, commands) = crossbeam_channel::unbounded();
        let (events, event_receiver) = crossbeam_channel::unbounded();
        let client = AsrSession::spawn(
            format!("ws://{address}/asr/v1"),
            AsrConfig::realtime_pcm16k(None),
            Vec::new(),
            commands,
            events,
        )
        .expect("ASR session thread should start");

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_notice = false;
        let mut saw_post_notice_response = false;
        while Instant::now() < deadline && !saw_post_notice_response {
            match event_receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("event should arrive")
            {
                AsrEvent::Notice { code, message } => {
                    assert_eq!(code, "gate_blocked");
                    assert_eq!(message, "登録した声と一致しませんでした。");
                    saw_notice = true;
                }
                AsrEvent::PartialText(tokens) if !tokens.is_empty() => {
                    assert_eq!(tokens[0].text, "次の応答");
                    saw_post_notice_response = true;
                }
                AsrEvent::Failed(error) => panic!("notice ended the session: {error}"),
                _ => {}
            }
        }
        assert!(saw_notice, "notice was not delivered");
        assert!(
            saw_post_notice_response,
            "session did not continue after notice"
        );

        to_session
            .send(AsrCommand::Stop)
            .expect("stop should be sent");
        let mut saw_finished = false;
        while Instant::now() < deadline && !saw_finished {
            match event_receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("finished event should arrive")
            {
                AsrEvent::Finished => saw_finished = true,
                AsrEvent::Failed(error) => panic!("session failed after notice: {error}"),
                _ => {}
            }
        }
        assert!(saw_finished, "session did not finish normally");
        client.join().expect("client thread should stop");
        server.join().expect("test server should stop");
    }
}
