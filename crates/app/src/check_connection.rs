use crate::settings::Settings;
use otoa_input_core::Readiness;
use otoa_input_platform::FRAME_SAMPLES;
use otoa_input_protocol::{
    AsrCommand, AsrConfig, AsrError, AsrEvent, AsrSession, POLICY_VIOLATION_CLOSE_CODE,
};
use std::fmt;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

const FINISHED_TIMEOUT: Duration = Duration::from_secs(15);
const SILENCE_FRAMES: usize = 9;
const POLICY_REJECTION_EXIT_CODE: i32 = 6;

pub fn run(settings: &Settings, provider: Arc<dyn otoa_input_core::ConnectionProvider>) -> i32 {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    match provider.readiness() {
        Readiness::Ready => {}
        Readiness::NeedsLogin { message } | Readiness::NeedsSetup { message } => {
            print_line(&mut output, format_args!("NG: {message}"));
            return 2;
        }
    }
    let endpoint = match provider.endpoint(&settings.core) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            print_line(&mut output, format_args!("NG: connect failed: {error}"));
            return 4;
        }
    };

    let config_key = endpoint
        .headers
        .is_empty()
        .then(|| endpoint.api_key.clone())
        .flatten();
    let mut config = AsrConfig::realtime_pcm16k(config_key);
    config.language_hints = settings.language_hints.clone();

    let (to_session, commands) = crossbeam_channel::unbounded();
    let (events, event_receiver) = crossbeam_channel::unbounded();
    let session_thread =
        match AsrSession::spawn(endpoint.url, config, endpoint.headers, commands, events) {
            Ok(session_thread) => session_thread,
            Err(error) => {
                print_line(&mut output, format_args!("NG: connect failed: {error}"));
                return 4;
            }
        };

    let result = wait_for_finished(
        to_session,
        event_receiver,
        endpoint.api_key.as_deref().unwrap_or_default(),
        &mut output,
    );
    if result != 5 {
        let _ = session_thread.join();
    }
    result
}

fn wait_for_finished(
    to_session: crossbeam_channel::Sender<AsrCommand>,
    events: crossbeam_channel::Receiver<AsrEvent>,
    api_key: &str,
    output: &mut dyn Write,
) -> i32 {
    let deadline = Instant::now() + FINISHED_TIMEOUT;
    let mut counts = EventCounts::default();
    let mut last_event = None;
    let mut probe_sent = false;
    let mut probe_response_received = false;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            print_timeout(output, last_event);
            return 5;
        }

        match events.recv_timeout(remaining) {
            Ok(event) => {
                if let Some(kind) = counts.record(&event) {
                    last_event = Some(kind);
                }
                match event {
                    AsrEvent::Connected if !probe_sent => {
                        if let Err(error) = send_silence_probe(&to_session) {
                            print_line(output, format_args!("NG: connect failed: {error}"));
                            return 4;
                        }
                        probe_sent = true;
                    }
                    AsrEvent::FinalText(_)
                    | AsrEvent::PartialText(_)
                    | AsrEvent::Endpoint
                    | AsrEvent::FinalizeDone
                        if probe_sent =>
                    {
                        probe_response_received = true;
                    }
                    AsrEvent::Closed {
                        code: Some(POLICY_VIOLATION_CLOSE_CODE),
                        reason,
                    } => {
                        print_policy_rejection(output, &reason);
                        return POLICY_REJECTION_EXIT_CODE;
                    }
                    AsrEvent::Finished if probe_sent && probe_response_received => {
                        print_line(output, "OK: connected, authenticated, finished");
                        print_line(output, counts);
                        return 0;
                    }
                    AsrEvent::Finished if probe_sent => {
                        print_line(
                            output,
                            "NG: connect failed: connection closed before receiving a response",
                        );
                        return 4;
                    }
                    AsrEvent::Failed(AsrError::Server {
                        code,
                        error_type,
                        request_id,
                        ..
                    }) => {
                        print_server_error(
                            output,
                            &error_type,
                            code,
                            request_id.as_deref(),
                            api_key,
                        );
                        return 3;
                    }
                    AsrEvent::Failed(error) => {
                        print_line(
                            output,
                            format_args!(
                                "NG: connect failed: {}",
                                redact_secret(&connection_error_reason(&error), api_key)
                            ),
                        );
                        return 4;
                    }
                    AsrEvent::Finished => {
                        print_line(output, "NG: connect failed: finished before connected");
                        return 4;
                    }
                    _ => {}
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                print_timeout(output, last_event);
                return 5;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                print_line(output, "NG: connect failed: event channel closed");
                return 4;
            }
        }
    }
}

fn send_silence_probe(
    to_session: &crossbeam_channel::Sender<AsrCommand>,
) -> Result<(), &'static str> {
    for _ in 0..SILENCE_FRAMES {
        to_session
            .send(AsrCommand::Audio(vec![0; FRAME_SAMPLES * 2]))
            .map_err(|_| "failed to send silence")?;
    }
    to_session
        .send(AsrCommand::Finalize)
        .map_err(|_| "failed to send finalize")?;
    to_session
        .send(AsrCommand::Stop)
        .map_err(|_| "failed to send stop")?;
    Ok(())
}

fn print_policy_rejection(output: &mut dyn Write, reason: &str) {
    if reason.is_empty() {
        print_line(
            output,
            format_args!(
                "NG: connection rejected by server (WebSocket close code {POLICY_VIOLATION_CLOSE_CODE})"
            ),
        );
    } else {
        print_line(output, format_args!("NG: {reason}"));
    }
}

fn print_server_error(
    output: &mut dyn Write,
    error_type: &str,
    code: u32,
    request_id: Option<&str>,
    api_key: &str,
) {
    print_line(
        output,
        format_args!(
            "NG: サーバーエラー {} ({})",
            redact_secret(error_type, api_key),
            code
        ),
    );
    print_line(
        output,
        format_args!(
            "request_id: {}",
            request_id
                .map(|request_id| redact_secret(request_id, api_key))
                .unwrap_or_else(|| "none".to_string())
        ),
    );
}

fn print_timeout(output: &mut dyn Write, last_event: Option<EventKind>) {
    print_line(output, "NG: timeout waiting for finished");
    print_line(
        output,
        format_args!(
            "last event: {}",
            last_event.map_or("none", EventKind::label)
        ),
    );
}

fn print_line(output: &mut dyn Write, message: impl fmt::Display) {
    let _ = writeln!(output, "{message}");
}

fn connection_error_reason(error: &AsrError) -> String {
    match error {
        AsrError::Connect(reason) | AsrError::Io(reason) | AsrError::Decode(reason) => {
            reason.clone()
        }
        AsrError::ClosedEarly => "connection closed by server before finished".to_string(),
        AsrError::Server { .. } => "server error".to_string(),
    }
}

fn redact_secret(value: &str, secret: &str) -> String {
    if secret.is_empty() {
        value.to_string()
    } else {
        value.replace(secret, "[redacted]")
    }
}

#[derive(Clone, Copy)]
enum EventKind {
    Connected,
    FinalText,
    PartialText,
    SpeechEndpoint,
    FinalizeDone,
    Finished,
    Notice,
    Failed,
}

impl EventKind {
    fn label(self) -> &'static str {
        match self {
            Self::Connected => "Connected",
            Self::FinalText => "FinalText",
            Self::PartialText => "PartialText",
            Self::SpeechEndpoint => "SpeechEndpoint",
            Self::FinalizeDone => "FinalizeDone",
            Self::Finished => "Finished",
            Self::Notice => "Notice",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Default)]
struct EventCounts([usize; 8]);

impl EventCounts {
    fn record(&mut self, event: &AsrEvent) -> Option<EventKind> {
        let kind = match event {
            AsrEvent::Connected => EventKind::Connected,
            AsrEvent::FinalText(_) => EventKind::FinalText,
            AsrEvent::PartialText(_) => EventKind::PartialText,
            AsrEvent::Endpoint => EventKind::SpeechEndpoint,
            AsrEvent::FinalizeDone => EventKind::FinalizeDone,
            AsrEvent::Finished => EventKind::Finished,
            AsrEvent::Closed { .. } => return None,
            // 名乗りは診断の数え上げでは通知と同じ扱いでよい。**利用者には出さない。**
            AsrEvent::Notice { .. } | AsrEvent::Backend(_) | AsrEvent::WarmupAfter(_) => {
                EventKind::Notice
            }
            AsrEvent::Failed(_) => EventKind::Failed,
        };
        self.0[kind as usize] += 1;
        Some(kind)
    }
}

impl fmt::Display for EventCounts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "events:")?;
        for (index, kind) in [
            EventKind::Connected,
            EventKind::FinalText,
            EventKind::PartialText,
            EventKind::SpeechEndpoint,
            EventKind::FinalizeDone,
            EventKind::Finished,
            EventKind::Notice,
            EventKind::Failed,
        ]
        .into_iter()
        .enumerate()
        {
            write!(formatter, " {}={},", kind.label(), self.0[index])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{wait_for_finished, POLICY_REJECTION_EXIT_CODE};
    use otoa_input_protocol::{AsrConfig, AsrEvent, AsrSession};
    use std::borrow::Cow;
    use std::net::TcpListener;
    use std::time::Duration;
    use tungstenite::{
        accept,
        protocol::{frame::coding::CloseCode, CloseFrame},
        Message,
    };

    fn check_result(events: impl IntoIterator<Item = AsrEvent>) -> (i32, String) {
        let (to_session, _commands) = crossbeam_channel::unbounded();
        let (event_sender, event_receiver) = crossbeam_channel::unbounded();
        for event in events {
            event_sender
                .send(event)
                .expect("test event should be accepted");
        }
        drop(event_sender);

        let mut output = Vec::new();
        let result = wait_for_finished(to_session, event_receiver, "", &mut output);
        (
            result,
            String::from_utf8(output).expect("test output should be UTF-8"),
        )
    }

    #[test]
    fn finished_after_a_probe_response_is_reported_as_success() {
        let (result, output) = check_result([
            AsrEvent::Connected,
            AsrEvent::PartialText(Vec::new()),
            AsrEvent::FinalizeDone,
            AsrEvent::Finished,
        ]);

        assert_eq!(result, 0);
        assert!(output.starts_with("OK: connected, authenticated, finished\n"));
    }

    #[test]
    fn close_without_a_probe_response_is_not_reported_as_success() {
        let (result, output) = check_result([
            AsrEvent::Connected,
            AsrEvent::Closed {
                code: Some(1000),
                reason: String::new(),
            },
            AsrEvent::Finished,
        ]);

        assert_eq!(result, 4);
        assert_eq!(
            output,
            "NG: connect failed: connection closed before receiving a response\n"
        );
    }

    #[test]
    fn policy_close_reason_is_reported_as_rejection() {
        const REASON: &str = "not allowed";

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

            loop {
                match websocket.read().expect("probe should arrive") {
                    Message::Binary(_) => {
                        websocket
                            .send(Message::Close(Some(CloseFrame {
                                code: CloseCode::Policy,
                                reason: Cow::Borrowed(REASON),
                            })))
                            .expect("policy close should be sent");
                        websocket
                            .get_mut()
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .expect("close acknowledgement timeout should be accepted");
                        loop {
                            match websocket.read() {
                                Ok(Message::Close(_)) | Err(_) => return,
                                Ok(_) => {}
                            }
                        }
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

        let mut output = Vec::new();
        let result = wait_for_finished(to_session, event_receiver, "", &mut output);

        client.join().expect("client thread should stop");
        server.join().expect("test server should stop");
        assert_eq!(result, POLICY_REJECTION_EXIT_CODE);
        assert_eq!(
            String::from_utf8(output).expect("test output should be UTF-8"),
            format!("NG: {REASON}\n")
        );
    }
}
