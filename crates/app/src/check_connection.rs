use crate::settings::Settings;
use otoa_input_core::Readiness;
use otoa_input_platform::FRAME_SAMPLES;
use otoa_input_protocol::{AsrCommand, AsrConfig, AsrError, AsrEvent, AsrSession};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

const FINISHED_TIMEOUT: Duration = Duration::from_secs(15);
const SILENCE_FRAMES: usize = 9;

pub fn run(settings: &Settings, provider: Arc<dyn otoa_input_core::ConnectionProvider>) -> i32 {
    match provider.readiness() {
        Readiness::Ready => {}
        Readiness::NeedsLogin { message } | Readiness::NeedsSetup { message } => {
            println!("NG: {message}");
            return 2;
        }
    }
    let endpoint = match provider.endpoint(&settings.core) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            println!("NG: connect failed: {error}");
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
                println!("NG: connect failed: {error}");
                return 4;
            }
        };

    let result = wait_for_finished(
        to_session,
        event_receiver,
        endpoint.api_key.as_deref().unwrap_or_default(),
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
) -> i32 {
    let deadline = Instant::now() + FINISHED_TIMEOUT;
    let mut counts = EventCounts::default();
    let mut last_event = None;
    let mut probe_sent = false;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            print_timeout(last_event);
            return 5;
        }

        match events.recv_timeout(remaining) {
            Ok(event) => {
                let kind = counts.record(&event);
                last_event = Some(kind);
                match event {
                    AsrEvent::Connected if !probe_sent => {
                        if let Err(error) = send_silence_probe(&to_session) {
                            println!("NG: connect failed: {error}");
                            return 4;
                        }
                        probe_sent = true;
                    }
                    AsrEvent::Finished if probe_sent => {
                        println!("OK: connected, authenticated, finished");
                        println!("{}", counts);
                        return 0;
                    }
                    AsrEvent::Failed(AsrError::Server {
                        code,
                        error_type,
                        request_id,
                        ..
                    }) => {
                        print_server_error(&error_type, code, request_id.as_deref(), api_key);
                        return 3;
                    }
                    AsrEvent::Failed(error) => {
                        println!(
                            "NG: connect failed: {}",
                            redact_secret(&connection_error_reason(&error), api_key)
                        );
                        return 4;
                    }
                    AsrEvent::Finished => {
                        println!("NG: connect failed: finished before connected");
                        return 4;
                    }
                    _ => {}
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                print_timeout(last_event);
                return 5;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                println!("NG: connect failed: event channel closed");
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

fn print_server_error(error_type: &str, code: u32, request_id: Option<&str>, api_key: &str) {
    println!(
        "NG: サーバーエラー {} ({})",
        redact_secret(error_type, api_key),
        code
    );
    println!(
        "request_id: {}",
        request_id
            .map(|request_id| redact_secret(request_id, api_key))
            .unwrap_or_else(|| "none".to_string())
    );
}

fn print_timeout(last_event: Option<EventKind>) {
    println!("NG: timeout waiting for finished");
    println!(
        "last event: {}",
        last_event.map_or("none", EventKind::label)
    );
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
    fn record(&mut self, event: &AsrEvent) -> EventKind {
        let kind = match event {
            AsrEvent::Connected => EventKind::Connected,
            AsrEvent::FinalText(_) => EventKind::FinalText,
            AsrEvent::PartialText(_) => EventKind::PartialText,
            AsrEvent::Endpoint => EventKind::SpeechEndpoint,
            AsrEvent::FinalizeDone => EventKind::FinalizeDone,
            AsrEvent::Finished => EventKind::Finished,
            // 名乗りは診断の数え上げでは通知と同じ扱いでよい。**利用者には出さない。**
            AsrEvent::Notice { .. } | AsrEvent::Backend(_) => EventKind::Notice,
            AsrEvent::Failed(_) => EventKind::Failed,
        };
        self.0[kind as usize] += 1;
        kind
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
