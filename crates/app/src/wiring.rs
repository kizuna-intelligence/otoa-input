use crate::controller::{
    Controller, ControllerCommand, OverlayKind, OverlayView, UiUpdate, VadControl, VadFrame,
    VadMessage,
};
use crate::settings::Settings;
use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use otoa_input_platform::AudioFrame;
use otoa_input_vad::SileroVad;
use std::path::PathBuf;
use std::thread;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewScenario {
    Splash,
    Connecting,
    Listening,
    Finalizing,
    Committed,
    Error,
    Login,
    Settings,
}

impl PreviewScenario {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "splash" => Some(Self::Splash),
            "connecting" => Some(Self::Connecting),
            "listening" => Some(Self::Listening),
            "finalizing" => Some(Self::Finalizing),
            "committed" => Some(Self::Committed),
            "error" => Some(Self::Error),
            "login" => Some(Self::Login),
            _ => None,
        }
    }
}

pub struct Runtime {
    pub commands: Sender<ControllerCommand>,
    vad_control: Option<Sender<VadControl>>,
    controller_thread: Option<thread::JoinHandle<()>>,
    vad_thread: Option<thread::JoinHandle<()>>,
    preview_stop: Option<Sender<()>>,
    preview_thread: Option<thread::JoinHandle<()>>,
    preview_settings: bool,
}

impl Runtime {
    pub fn shutdown(mut self) {
        let _ = self.commands.send(ControllerCommand::Shutdown);
        if let Some(controller_thread) = self.controller_thread.take() {
            let _ = controller_thread.join();
        }
        if let Some(stop) = self.preview_stop.take() {
            let _ = stop.send(());
        }
        if let Some(preview_thread) = self.preview_thread.take() {
            let _ = preview_thread.join();
        }
        if let Some(vad_control) = self.vad_control {
            let _ = vad_control.send(VadControl::Shutdown);
        }
        if let Some(vad_thread) = self.vad_thread.take() {
            let _ = vad_thread.join();
        }
    }

    pub(crate) fn is_settings_preview(&self) -> bool {
        self.preview_settings
    }
}

pub fn start(
    settings: Settings,
    provider: std::sync::Arc<dyn otoa_input_core::ConnectionProvider>,
    bundled_server_failure: Option<String>,
    to_ui: Sender<UiUpdate>,
) -> Result<Runtime> {
    let (audio_sink, audio_rx) = crossbeam_channel::bounded::<AudioFrame>(64);
    let (vad_event_sink, vad_events) = crossbeam_channel::unbounded::<VadMessage>();
    let (vad_control, vad_control_rx) = crossbeam_channel::unbounded::<VadControl>();
    let (commands, command_rx) = crossbeam_channel::unbounded();

    let vad_model = resolve_vad_model_path(&settings);
    let vad_thread = spawn_vad_thread(vad_model, audio_rx, vad_event_sink, vad_control_rx)?;
    let controller_vad_control = vad_control.clone();
    let controller_thread = thread::Builder::new()
        .name("otoa-controller".to_string())
        .spawn(move || {
            match Controller::new(
                settings,
                provider,
                bundled_server_failure,
                to_ui,
                audio_sink,
                controller_vad_control,
                vad_events,
            ) {
                Ok(controller) => controller.run(command_rx),
                Err(error) => tracing::error!("failed to initialize controller: {error:#}"),
            }
        })?;

    Ok(Runtime {
        commands,
        vad_control: Some(vad_control),
        controller_thread: Some(controller_thread),
        vad_thread: Some(vad_thread),
        preview_stop: None,
        preview_thread: None,
        preview_settings: false,
    })
}

pub fn start_preview(
    _settings: Settings,
    scenario: PreviewScenario,
    to_ui: Sender<UiUpdate>,
) -> Result<Runtime> {
    let (commands, _command_rx) = crossbeam_channel::unbounded();
    let (stop, stop_rx) = crossbeam_channel::bounded(1);
    let preview_thread = thread::Builder::new()
        .name("otoa-preview".to_string())
        .spawn(move || {
            send_preview_update(&to_ui, scenario);
            loop {
                crossbeam_channel::select! {
                    recv(stop_rx) -> _ => break,
                    default(std::time::Duration::from_millis(100)) => {
                        send_preview_update(&to_ui, scenario);
                    }
                }
            }
        })?;

    Ok(Runtime {
        commands,
        vad_control: None,
        controller_thread: None,
        vad_thread: None,
        preview_stop: Some(stop),
        preview_thread: Some(preview_thread),
        preview_settings: matches!(scenario, PreviewScenario::Settings),
    })
}

fn send_preview_update(to_ui: &Sender<UiUpdate>, scenario: PreviewScenario) {
    let view = match scenario {
        PreviewScenario::Splash => OverlayView::Splash,
        PreviewScenario::Settings => OverlayView::Hidden,
        PreviewScenario::Connecting => preview_overlay(OverlayKind::Connecting),
        PreviewScenario::Listening => preview_overlay(OverlayKind::Recognizing),
        PreviewScenario::Finalizing => preview_overlay(OverlayKind::Finalizing),
        PreviewScenario::Committed => preview_overlay(OverlayKind::Committed),
        PreviewScenario::Error => preview_overlay(OverlayKind::Error),
        PreviewScenario::Login => preview_overlay(OverlayKind::LoginNeeded),
    };
    let _ = to_ui.send(UiUpdate::Overlay(view));
    let login_state = if matches!(scenario, PreviewScenario::Login) {
        crate::controller::LoginState::LoggedOut
    } else {
        crate::controller::LoginState::LoggedIn {
            email: "preview@example.invalid".to_string(),
        }
    };
    let _ = to_ui.send(UiUpdate::LoginState(login_state));
    let _ = to_ui.send(UiUpdate::Level {
        peak: if matches!(scenario, PreviewScenario::Listening) {
            (0.5 * f64::from(i16::MAX)) as i16
        } else {
            0
        },
        status: crate::controller::LevelStatus::Normal,
    });
}

fn preview_overlay(kind: crate::controller::OverlayKind) -> OverlayView {
    OverlayView::Shown {
        kind,
        committed: String::new(),
        partial: String::new(),
        error: String::new(),
    }
}

fn spawn_vad_thread(
    model_path: Option<PathBuf>,
    audio_rx: Receiver<AudioFrame>,
    events: Sender<VadMessage>,
    controls: Receiver<VadControl>,
) -> Result<thread::JoinHandle<()>> {
    Ok(thread::Builder::new()
        .name("otoa-vad".to_string())
        .spawn(move || run_vad_thread(model_path, audio_rx, events, controls))?)
}

fn run_vad_thread(
    model_path: Option<PathBuf>,
    audio_rx: Receiver<AudioFrame>,
    events: Sender<VadMessage>,
    controls: Receiver<VadControl>,
) {
    // 指定が無ければ埋め込んだモデルを使う。外部ファイルは要らない。
    let loaded = match &model_path {
        Some(path) => SileroVad::from_model_path(path),
        None => SileroVad::bundled(),
    };
    let mut vad = match loaded {
        Ok(vad) => vad,
        Err(error) => {
            let origin = model_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "同梱モデル".to_string());
            let _ = events.send(VadMessage::Failed(format!(
                "failed to load VAD model {origin}: {error:#}"
            )));
            return;
        }
    };
    let mut enabled = false;

    loop {
        if !enabled {
            match controls.recv() {
                Ok(VadControl::Resume) => {
                    vad.reset();
                    enabled = true;
                }
                Ok(VadControl::Suspend) => {
                    vad.reset();
                    drain_audio(&audio_rx);
                }
                Ok(VadControl::Shutdown) | Err(_) => return,
            }
            continue;
        }

        crossbeam_channel::select! {
            recv(controls) -> control => {
                match control {
                    Ok(VadControl::Resume) => vad.reset(),
                    Ok(VadControl::Suspend) => {
                        vad.reset();
                        enabled = false;
                        drain_audio(&audio_rx);
                    }
                    Ok(VadControl::Shutdown) | Err(_) => return,
                }
            }
            recv(audio_rx) -> frame => {
                let Ok(AudioFrame(samples)) = frame else { return };
                let mut probs = Vec::new();
                if let Err(error) = vad.push(&samples, &mut probs) {
                    let _ = events.send(VadMessage::Failed(format!("VAD inference failed: {error:#}")));
                    return;
                }
                if events.send(VadMessage::Frame(VadFrame { probs, samples })).is_err() {
                    return;
                }
            }
        }
    }
}

fn drain_audio(audio_rx: &Receiver<AudioFrame>) {
    while audio_rx.try_recv().is_ok() {}
}

/// 差し替え用の VAD モデルの場所。指定が無ければ `None` を返し、
/// バイナリへ埋め込んだモデルを使う。
fn resolve_vad_model_path(settings: &Settings) -> Option<PathBuf> {
    if !settings.vad_model_path.is_empty() {
        return Some(PathBuf::from(&settings.vad_model_path));
    }
    std::env::var_os("OTOA_VAD_MODEL")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}
