use crate::controller::{
    Controller, ControllerCommand, UiUpdate, VadControl, VadFrame, VadMessage,
};
use crate::settings::Settings;
use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use otoa_input_platform::AudioFrame;
use otoa_input_vad::SileroVad;
use std::path::PathBuf;
use std::thread;

pub struct Runtime {
    pub commands: Sender<ControllerCommand>,
    vad_control: Sender<VadControl>,
    controller_thread: Option<thread::JoinHandle<()>>,
    vad_thread: Option<thread::JoinHandle<()>>,
}

impl Runtime {
    pub fn shutdown(mut self) {
        let _ = self.commands.send(ControllerCommand::Shutdown);
        if let Some(controller_thread) = self.controller_thread.take() {
            let _ = controller_thread.join();
        }
        let _ = self.vad_control.send(VadControl::Shutdown);
        if let Some(vad_thread) = self.vad_thread.take() {
            let _ = vad_thread.join();
        }
    }
}

pub fn start(
    settings: Settings,
    provider: std::sync::Arc<dyn otoa_input_core::ConnectionProvider>,
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
        vad_control,
        controller_thread: Some(controller_thread),
        vad_thread: Some(vad_thread),
    })
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
