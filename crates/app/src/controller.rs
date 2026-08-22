use crate::settings::Settings;
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use otoa_input_core::Account;
use otoa_input_core::{
    ConnectionProvider, GateEvent, PreRoll, Readiness, Session, SessionInput, SessionState,
    SpeechGate, Transcript,
};
use otoa_input_platform::{AudioCapture, AudioFrame, PasteMethod, TextOutput};
use otoa_input_protocol::{
    AsrCommand, AsrConfig, AsrError, AsrEvent, AsrSession, AsrToken, EndpointTuning,
};
use otoa_input_vad::{VAD_FRAME_MS, VAD_SAMPLE_RATE};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const PENDING_AUDIO_LIMIT: usize = 100;
const CONTROLLER_TICK: Duration = Duration::from_millis(100);
const TEXT_UI_MIN_INTERVAL: Duration = Duration::from_millis(30);
const AUDIO_PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(1);
const DUPLICATE_COMMIT_WINDOW: Duration = Duration::from_secs(5);
const OVERLAY_ERROR_DURATION: Duration = Duration::from_secs(8);
const FAILED_RETRY_INITIAL: Duration = Duration::from_secs(5);
const FAILED_RETRY_MAX: Duration = Duration::from_secs(30);
#[allow(dead_code)]
const CONNECTING_TIMEOUT: Duration = Duration::from_secs(10);
#[allow(dead_code)]
const CLOSING_TIMEOUT: Duration = Duration::from_secs(8);
const GATEWAY_URL_MISSING_MESSAGE: &str =
    "ゲートウェイURLが設定されていません。設定画面の「詳細」で指定してください。";

#[derive(Debug, Clone, PartialEq)]
pub enum OverlayView {
    Hidden,
    Splash,
    Shown {
        kind: OverlayKind,
        committed: String,
        partial: String,
        error: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverlayKind {
    Connecting,
    /// マイクが今の発話を拾っている。
    Recognizing,
    /// 発話は終わり、`finalize` の結果を待っている。
    /// この状態を持たないと、認識待ちの間だけオーバーレイが消えて、
    /// 何も起きていないように見える。
    Finalizing,
    Committed,
    Error,
    LoginNeeded,
}

#[derive(Debug, Clone)]
pub enum UiUpdate {
    State(SessionState),
    Overlay(OverlayView),
    Level { peak: i16, status: LevelStatus },
    Account { email: Option<String> },
    LoginState(LoginState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelStatus {
    Normal,
    TooQuiet,
    Clipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginState {
    NotRequired,
    LoggedOut,
    InProgress,
    LoggedIn { email: String },
    Failed { reason: String },
}

#[derive(Debug)]
pub struct VadFrame {
    pub probs: Vec<f32>,
    pub samples: Vec<i16>,
}

pub enum VadMessage {
    Frame(VadFrame),
    Failed(String),
}

pub enum VadControl {
    Resume,
    Suspend,
    Shutdown,
}

pub enum ControllerCommand {
    StartStop,
    StartLogin,
    Logout,
    UpdateSettings(Box<Settings>),
    Shutdown,
}

pub struct Controller {
    pub(crate) session: Session,
    pub(crate) transcript: Transcript,
    pub(crate) settings: Settings,
    pub(crate) pending_audio: Vec<Vec<u8>>,
    pub(crate) to_asr: Option<Sender<AsrCommand>>,
    pub(crate) to_ui: Sender<UiUpdate>,
    pub(crate) text_out: TextOutput,
    overlay: OverlayView,
    overlay_error_until: Option<Instant>,
    splash_started_at: Option<Instant>,
    gate: SpeechGate,
    preroll: PreRoll,
    /// ASR セッションを開始した時刻。
    session_started_at: Option<Instant>,
    /// Connecting へ遷移した時刻。
    #[allow(dead_code)]
    connecting_started_at: Option<Instant>,
    /// Closing へ遷移した時刻。
    #[allow(dead_code)]
    closing_started_at: Option<Instant>,
    /// ASR サーバーから最後に発話区切り（`<end>`）を受信した時刻。
    last_speech_endpoint_at: Option<Instant>,
    /// 現在の ASR セッションへ送った音声の累計時間。
    sent_audio_ms: u64,
    /// Stop を送信済み、または Finished を受信済みで終了処理中。
    asr_closing: bool,
    /// endpoint_mode=client のとき、finalize を送ってから次の発話開始までの抑止。
    /// これが無いと SpeechEnded のたびに finalize を送り、同じ確定を繰り返す。
    client_finalize_sent: bool,
    /// `finalize` を送ってから結果が返るまで。オーバーレイの「認識中」表示に使う。
    finalize_pending: bool,
    /// Failed に入った時刻。自動復帰対象でない失敗では使わない。
    failed_at: Option<Instant>,
    /// Failed からの自動復帰を許可するか。
    failed_recovery_enabled: bool,
    /// Failed から復帰するまでの待機時間。
    failed_retry_delay: Duration,
    sent_level_sum_squares: u64,
    sent_level_peak: i16,
    sent_level_samples: u64,
    sent_frames: u64,
    /// 音声累計ログを最後に出した時刻。
    last_audio_log_at: Option<Instant>,
    vad_level_sum_squares: u64,
    vad_level_peak: i16,
    vad_level_samples: u64,
    vad_prob_max: f32,
    level_peak: i16,
    level_clip_window: VecDeque<LevelWindow>,
    last_vad_log_at: Instant,
    audio_sink: Sender<AudioFrame>,
    vad_control: Sender<VadControl>,
    vad_events: Receiver<VadMessage>,
    vad_channel_open: bool,
    asr_events: Option<Receiver<AsrEvent>>,
    asr_thread: Option<thread::JoinHandle<()>>,
    pending_commit: String,
    committed_hold_until: Option<Instant>,
    last_commit: Option<(String, Instant)>,
    last_text_ui: Option<Instant>,
    audio_capture: Option<AudioCapture>,
    pending_settings: Option<Settings>,
    active_api_key: Option<String>,
    provider: Arc<dyn ConnectionProvider>,
    login_cancel: Option<Arc<AtomicBool>>,
    login_result_rx: Option<Receiver<anyhow::Result<()>>>,
    login_thread: Option<thread::JoinHandle<()>>,
}

impl Controller {
    pub fn new(
        settings: Settings,
        provider: Arc<dyn ConnectionProvider>,
        to_ui: Sender<UiUpdate>,
        audio_sink: Sender<AudioFrame>,
        vad_control: Sender<VadControl>,
        vad_events: Receiver<VadMessage>,
    ) -> anyhow::Result<Self> {
        let gate = gate_from_settings(&settings);
        let preroll = PreRoll::new(milliseconds_to_samples(settings.preroll_ms));
        Ok(Self {
            session: Session::new(),
            transcript: Transcript::new(),
            settings,
            pending_audio: Vec::new(),
            to_asr: None,
            to_ui,
            text_out: TextOutput::new()?,
            overlay: OverlayView::Splash,
            overlay_error_until: None,
            splash_started_at: Some(Instant::now()),
            gate,
            preroll,
            session_started_at: None,
            connecting_started_at: None,
            closing_started_at: None,
            last_speech_endpoint_at: None,
            sent_audio_ms: 0,
            asr_closing: false,
            client_finalize_sent: false,
            finalize_pending: false,
            failed_at: None,
            failed_recovery_enabled: false,
            failed_retry_delay: FAILED_RETRY_INITIAL,
            sent_level_sum_squares: 0,
            sent_level_peak: 0,
            sent_level_samples: 0,
            sent_frames: 0,
            last_audio_log_at: None,
            vad_level_sum_squares: 0,
            vad_level_peak: 0,
            vad_level_samples: 0,
            vad_prob_max: 0.0,
            level_peak: 0,
            level_clip_window: VecDeque::new(),
            last_vad_log_at: Instant::now(),
            audio_sink,
            vad_control,
            vad_events,
            vad_channel_open: true,
            asr_events: None,
            asr_thread: None,
            pending_commit: String::new(),
            committed_hold_until: None,
            last_commit: None,
            last_text_ui: None,
            audio_capture: None,
            pending_settings: None,
            active_api_key: None,
            provider,
            login_cancel: None,
            login_result_rx: None,
            login_thread: None,
        })
    }

    pub fn run(mut self, commands: Receiver<ControllerCommand>) {
        self.send_ui(UiUpdate::State(self.session.state()));
        self.send_account_update();
        self.send_login_state();
        self.send_ui(UiUpdate::Overlay(self.overlay.clone()));
        if self.settings.listening_enabled && !self.connection_needs_attention() {
            self.enable_listening();
        } else {
            self.suspend_vad();
        }

        let ticker = crossbeam_channel::tick(CONTROLLER_TICK);
        let mut shutting_down = false;
        while !shutting_down {
            self.drain_vad_events();
            self.drain_asr_events();
            self.drain_login_events();

            crossbeam_channel::select! {
                recv(commands) -> command => {
                    match command {
                        Ok(ControllerCommand::StartStop) => self.toggle_listening(),
                        Ok(ControllerCommand::StartLogin) => self.start_login(),
                        Ok(ControllerCommand::Logout) => self.logout(),
                        Ok(ControllerCommand::UpdateSettings(settings)) => {
                            self.update_settings(*settings)
                        }
                        Ok(ControllerCommand::Shutdown) | Err(_) => {
                            shutting_down = true;
                        }
                    }
                }
                recv(ticker) -> _ => self.periodic(),
                default(Duration::from_millis(10)) => {}
            }
        }

        self.request_shutdown();
        self.cleanup_asr();
        self.cancel_login();
    }

    fn toggle_listening(&mut self) {
        if self.session.state() == SessionState::Disabled {
            {
                if self.login_required() {
                    self.start_login();
                } else {
                    self.enable_listening();
                }
            }
        } else {
            self.disable_listening();
        }
    }

    fn start_login(&mut self) {
        if self.login_thread.is_some() {
            self.send_login_state_update(LoginState::InProgress);
            self.show_overlay_error("ログイン処理中…ブラウザで操作してください".to_string());
            return;
        }

        if self.provider.prepare().is_none() {
            self.send_login_state();
            self.show_overlay_error("この接続ではログインは不要です".to_string());
            return;
        }

        let (result_tx, result_rx) = crossbeam_channel::bounded(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancelled.clone();
        let provider = Arc::clone(&self.provider);
        let worker = thread::Builder::new()
            .name("otoa-login".to_string())
            .spawn(move || {
                let result = provider.authenticate(&worker_cancel);
                let _ = result_tx.send(result);
            });

        let worker = match worker {
            Ok(worker) => worker,
            Err(error) => {
                self.send_login_failure(format!("ログイン処理を開始できません: {error}"));
                return;
            }
        };
        self.login_cancel = Some(cancelled);
        self.login_result_rx = Some(result_rx);
        self.login_thread = Some(worker);
        self.send_login_state_update(LoginState::InProgress);
        self.show_overlay_error("ログイン処理中…ブラウザで操作してください".to_string());
    }

    fn drain_login_events(&mut self) {
        let result = match self.login_result_rx.as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => Some(result),
            Some(Err(TryRecvError::Empty)) => None,
            Some(Err(TryRecvError::Disconnected)) => {
                Some(Err(anyhow::anyhow!("ログイン処理が予期せず終了しました")))
            }
            None => None,
        };
        let Some(result) = result else {
            return;
        };
        self.finish_login_worker();
        match result {
            Ok(()) => {
                self.send_account_update();
                self.send_login_state();
                self.hide_overlay();
                if self.settings.listening_enabled && self.session.state() == SessionState::Disabled
                {
                    self.enable_listening();
                }
            }
            Err(error) if error.to_string().contains("timed out") => {
                self.send_account_update();
                self.send_login_state();
                self.show_overlay_error("ログインがタイムアウトしました".to_string());
            }
            Err(error) => self.send_login_failure(short_login_error(&error)),
        }
    }

    fn finish_login_worker(&mut self) {
        self.login_result_rx = None;
        self.login_cancel = None;
        if let Some(worker) = self.login_thread.take() {
            let _ = worker.join();
        }
    }

    fn cancel_login(&mut self) {
        if let Some(cancelled) = &self.login_cancel {
            cancelled.store(true, std::sync::atomic::Ordering::Release);
        }
        self.finish_login_worker();
    }

    fn send_login_failure(&mut self, reason: String) {
        self.send_login_state_update(LoginState::Failed {
            reason: reason.clone(),
        });
        self.show_overlay_error(format!("ログイン失敗: {reason}"));
    }

    fn send_login_state(&self) {
        let state = match self.provider.account() {
            Some(Account { email }) => LoginState::LoggedIn {
                email: email.unwrap_or_else(|| "ログイン済み".to_string()),
            },
            None if self.provider.prepare().is_some() => LoginState::LoggedOut,
            None => LoginState::NotRequired,
        };
        self.send_login_state_update(state);
    }

    fn send_login_state_update(&self, state: LoginState) {
        self.send_ui(UiUpdate::LoginState(state));
    }

    fn send_account_update(&self) {
        self.send_ui(UiUpdate::Account {
            email: self.provider.account().and_then(|account| account.email),
        });
    }

    fn require_connection(&mut self) {
        let readiness = self.provider.readiness();
        let (kind, message) = match readiness {
            Readiness::NeedsLogin { message } => (OverlayKind::LoginNeeded, message),
            Readiness::NeedsSetup { message } => (OverlayKind::Error, message),
            Readiness::Ready => return,
        };
        tracing::info!(target: "otoa_input", %message, "connection is not ready");
        if self.session.state() != SessionState::Disabled {
            self.suspend_vad();
            self.audio_capture.take();
            self.send_stop();
            self.cleanup_asr();
            if self.session.state() != SessionState::Failed {
                let _ = self.session.apply(SessionInput::Failed);
            }
            let _ = self.session.apply(SessionInput::Disable);
        }
        self.send_ui(UiUpdate::State(SessionState::Disabled));
        self.overlay_error_until = None;
        self.set_overlay(OverlayView::Shown {
            kind,
            committed: String::new(),
            partial: String::new(),
            error: message,
        });
        self.send_login_state();
    }

    fn logout(&mut self) {
        if let Err(error) = self.provider.logout() {
            self.report_error(format!("ログアウトに失敗しました: {error:#}"));
            return;
        }
        self.send_account_update();
        self.send_login_state();
        self.disable_listening();
        self.require_connection();
    }

    fn enable_listening(&mut self) {
        if self.connection_needs_attention() {
            self.require_connection();
            return;
        }
        if !self.session.apply(SessionInput::Enable) {
            return;
        }
        self.failed_at = None;
        self.failed_recovery_enabled = false;
        self.failed_retry_delay = FAILED_RETRY_INITIAL;
        // 待受を始める時点より前の音声は残しておく意味がない。
        self.preroll.clear();
        self.reset_vad_state();
        if let Err(error) = self.start_audio_capture() {
            self.suspend_vad();
            self.fail_runtime_user_action(format!("failed to start microphone: {error:#}"));
            return;
        }
        self.send_ui(UiUpdate::State(SessionState::Listening));
    }

    fn disable_listening(&mut self) {
        self.hide_overlay();
        self.suspend_vad();
        self.audio_capture.take();
        self.gate.reset();
        self.preroll.clear();
        self.level_clip_window.clear();

        match self.session.state() {
            SessionState::Listening => {
                if self.session.apply(SessionInput::Disable) {
                    self.send_ui(UiUpdate::State(SessionState::Disabled));
                }
            }
            SessionState::Connecting | SessionState::Streaming | SessionState::Holding => {
                if self.session.apply(SessionInput::Disable) {
                    self.closing_started_at = Some(Instant::now());
                    self.send_ui(UiUpdate::State(SessionState::Closing));
                    self.send_finalize();
                    self.send_stop();
                }
            }
            SessionState::Failed => {
                if self.session.apply(SessionInput::Disable) {
                    self.failed_at = None;
                    self.failed_recovery_enabled = false;
                    self.send_ui(UiUpdate::State(SessionState::Disabled));
                }
            }
            SessionState::Closing => {
                self.session.apply(SessionInput::Disable);
            }
            SessionState::Disabled => {}
        }
    }

    fn update_settings(&mut self, settings: Settings) {
        let microphone_changed = self.settings.microphone != settings.microphone;
        let product_settings = settings.product_settings_value();
        self.provider
            .update_settings(&settings.core, product_settings.as_ref());
        if matches!(
            self.session.state(),
            SessionState::Disabled | SessionState::Failed
        ) {
            let should_enable = self.session.state() == SessionState::Disabled
                && !self.settings.listening_enabled
                && settings.listening_enabled;
            self.settings = settings;
            self.rebuild_vad_configuration();
            if should_enable {
                self.enable_listening();
            }
        } else {
            if microphone_changed && self.audio_capture.take().is_some() {
                self.settings.microphone = settings.microphone.clone();
                if let Err(error) = self.start_audio_capture() {
                    self.fail_runtime_user_action(format!(
                        "failed to switch microphone: {error:#}"
                    ));
                }
            }
            self.pending_settings = Some(settings);
        }
    }

    fn start_audio_capture(&mut self) -> anyhow::Result<()> {
        if self.audio_capture.is_some() {
            return Ok(());
        }
        let microphone =
            (!self.settings.microphone.is_empty()).then_some(self.settings.microphone.as_str());
        let capture = AudioCapture::start(microphone, self.audio_sink.clone())?;
        self.audio_capture = Some(capture);
        Ok(())
    }

    fn handle_vad_frame(&mut self, frame: VadFrame) {
        let samples = apply_input_gain(&frame.samples, self.settings.input_gain);
        self.level_peak = samples.iter().copied().map(sample_level).max().unwrap_or(0);
        self.record_level_window(&samples);
        self.record_vad_level(&samples, &frame.probs);
        if !self.session.is_listening() {
            return;
        }

        let events = frame
            .probs
            .into_iter()
            .filter_map(|prob| self.gate.push(prob))
            .collect::<Vec<_>>();
        for event in events {
            self.handle_gate_event(event);
        }
        self.handle_vad_samples(&samples);
    }

    fn handle_gate_event(&mut self, event: GateEvent) {
        match event {
            GateEvent::SpeechStarted => {
                self.client_finalize_sent = false;
                self.clear_commit_hold();
                match self.session.state() {
                    SessionState::Listening => {
                        if !self.session.apply(SessionInput::SpeechStarted) {
                            return;
                        }
                        let now = Instant::now();
                        self.session_started_at = Some(now);
                        self.connecting_started_at = Some(now);
                        self.closing_started_at = None;
                        self.last_speech_endpoint_at = Some(now);
                        self.sent_audio_ms = 0;
                        self.last_audio_log_at = Some(now);
                        self.log_session_event("SpeechStarted");
                        let preroll = self.preroll.take();
                        // 先頭欠けの切り分け用。検知が遅れた分をプリロールが
                        // 覆えているかは、この値と preroll_ms の設定で分かる。
                        tracing::debug!(
                            target: "otoa_input",
                            preroll_ms = (preroll.len() * 1000) / VAD_SAMPLE_RATE as usize,
                            capacity_ms = self.settings.preroll_ms,
                            "session preroll"
                        );
                        if let Err(error) = self.start_asr(preroll) {
                            if self.session.state() != SessionState::Disabled {
                                let message = error.to_string();
                                if is_user_action_failure_message(&message) {
                                    self.fail_runtime_user_action(message);
                                } else {
                                    self.fail_runtime(message);
                                }
                            }
                        }
                    }
                    SessionState::Streaming => self.refresh_overlay(),
                    _ => {}
                }
            }
            GateEvent::SpeechEnded => {
                // endpoint_mode = "client" のときは、端末の VAD が終話を決める。
                // 区切りの決定者はここ 1 か所。サーバー側にも持たせない。
                if self.settings.endpoint_mode == "client"
                    && self.session.state() == SessionState::Streaming
                    && !self.client_finalize_sent
                {
                    self.client_finalize_sent = true;
                    if self.send_finalize() {
                        self.finalize_pending = true;
                    }
                }
                self.refresh_overlay()
            }
        }
    }

    fn handle_vad_samples(&mut self, samples: &[i16]) {
        match self.session.state() {
            SessionState::Listening | SessionState::Holding | SessionState::Failed => {
                self.preroll.push(samples);
            }
            SessionState::Connecting => {
                self.queue_pending_audio(samples_to_bytes(samples));
            }
            SessionState::Streaming => {
                self.send_audio(samples);
            }
            // 接続を閉じている間も音声は捨てない。ここで捨てると、閉じている
            // 最中に話し始めた分がプリロールにも残らず、次の接続の先頭が欠ける。
            SessionState::Closing => {
                self.preroll.push(samples);
            }
            // マイクを止めている間だけ捨てる。
            SessionState::Disabled => {}
        }
    }

    fn start_asr(&mut self, preroll: Vec<i16>) -> anyhow::Result<()> {
        let endpoint = self.provider.endpoint(&self.settings.core)?;

        let config_key = endpoint
            .headers
            .is_empty()
            .then(|| endpoint.api_key.clone())
            .flatten();
        let mut config = AsrConfig::realtime_pcm16k(config_key)
            .with_endpoint_mode(&self.settings.endpoint_mode)
            .with_endpoint_tuning(EndpointTuning {
                max_delay_ms: self.settings.endpoint_max_delay_ms,
                sensitivity: self.settings.endpoint_sensitivity,
                latency_level: self.settings.endpoint_latency_level,
            });
        config.language_hints = self.settings.language_hints.clone();
        let (to_asr, commands) = crossbeam_channel::unbounded();
        let (events, asr_events) = crossbeam_channel::unbounded();
        let asr_thread =
            AsrSession::spawn(endpoint.url, config, endpoint.headers, commands, events);

        self.active_api_key = endpoint.api_key;
        self.asr_closing = false;
        self.to_asr = Some(to_asr);
        self.asr_events = Some(asr_events);
        self.asr_thread = Some(asr_thread);
        self.pending_audio.clear();
        if !preroll.is_empty() {
            self.pending_audio.push(samples_to_bytes(&preroll));
        }
        self.overlay_error_until = None;
        self.set_overlay(OverlayView::Shown {
            kind: OverlayKind::Connecting,
            committed: String::new(),
            partial: String::new(),
            error: String::new(),
        });
        self.send_ui(UiUpdate::State(SessionState::Connecting));
        Ok(())
    }

    fn queue_pending_audio(&mut self, bytes: Vec<u8>) {
        if self.pending_audio.len() >= PENDING_AUDIO_LIMIT {
            self.pending_audio.remove(0);
        }
        self.pending_audio.push(bytes);
    }

    fn send_audio(&mut self, samples: &[i16]) {
        let _ = self.send_audio_bytes(samples_to_bytes(samples));
    }

    fn send_audio_bytes(&mut self, bytes: Vec<u8>) -> bool {
        let Some(to_asr) = self.to_asr.clone() else {
            self.fail_runtime("音声認識セッションが利用できません".to_string());
            return false;
        };
        if let Err(error) = to_asr.send(AsrCommand::Audio(bytes.clone())) {
            self.fail_runtime(format!("音声の送信に失敗しました: {error}"));
            return false;
        }
        for sample in bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        {
            self.sent_level_sum_squares += u64::from(i32::from(sample).unsigned_abs().pow(2));
            self.sent_level_peak = self.sent_level_peak.max(sample_level(sample));
        }
        self.sent_level_samples += (bytes.len() / 2) as u64;
        self.sent_frames += 1;
        self.sent_audio_ms += (bytes.len() as u64 / 2 * 1000) / u64::from(VAD_SAMPLE_RATE);
        true
    }

    fn send_finalize(&mut self) -> bool {
        let Some(to_asr) = self.to_asr.clone() else {
            self.fail_runtime("音声認識セッションが利用できません".to_string());
            return false;
        };
        if let Err(error) = to_asr.send(AsrCommand::Finalize) {
            self.fail_runtime(format!("音声認識セッションの終了に失敗しました: {error}"));
            return false;
        }
        true
    }

    fn send_stop(&mut self) {
        if let Some(to_asr) = self.to_asr.clone() {
            self.asr_closing = true;
            if let Err(error) = to_asr.send(AsrCommand::Stop) {
                tracing::debug!("failed to stop ASR session: {error}");
            }
        }
    }

    fn drain_vad_events(&mut self) {
        if !self.vad_channel_open {
            return;
        }
        for _ in 0..32 {
            match self.vad_events.try_recv() {
                Ok(VadMessage::Frame(frame)) => self.handle_vad_frame(frame),
                Ok(VadMessage::Failed(message)) => {
                    self.vad_channel_open = false;
                    self.handle_vad_failure(message);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.vad_channel_open = false;
                    self.handle_vad_failure("VAD worker stopped unexpectedly".to_string());
                    break;
                }
            }
        }
    }

    fn handle_vad_failure(&mut self, message: String) {
        self.suspend_vad();
        self.audio_capture.take();
        self.fail_runtime(message);
    }

    fn drain_asr_events(&mut self) {
        loop {
            let next = self.asr_events.as_ref().map(|events| events.try_recv());
            match next {
                Some(Ok(event)) => self.handle_asr_event(event),
                Some(Err(TryRecvError::Empty)) => break,
                Some(Err(TryRecvError::Disconnected)) => {
                    if self.asr_closing || self.session.state() == SessionState::Closing {
                        tracing::debug!("ASR event channel closed during normal shutdown");
                        self.finish_asr_shutdown();
                    } else {
                        self.fail_runtime("音声認識セッションが予期せず終了しました".to_string());
                    }
                    self.asr_events = None;
                    break;
                }
                None => break,
            }
        }
    }

    fn handle_asr_event(&mut self, event: AsrEvent) {
        match event {
            AsrEvent::Connected => {
                if !self.session.apply(SessionInput::Connected) {
                    return;
                }
                self.failed_at = None;
                self.failed_recovery_enabled = false;
                self.failed_retry_delay = FAILED_RETRY_INITIAL;
                self.connecting_started_at = None;
                let pending_audio = std::mem::take(&mut self.pending_audio);
                for bytes in pending_audio {
                    if !self.send_audio_bytes(bytes) {
                        return;
                    }
                }
                self.send_ui(UiUpdate::State(SessionState::Streaming));
                self.refresh_overlay();
                self.log_session_event("Connected");
            }
            AsrEvent::FinalText(tokens) => {
                self.transcript.push_final(&tokens_to_text(&tokens));
                self.send_text_update(true);
            }
            AsrEvent::PartialText(tokens) => {
                let text = tokens_to_text(&tokens);
                let had_commit_hold = !text.is_empty() && self.clear_commit_hold();
                self.transcript.replace_partial(&text);
                self.send_text_update(had_commit_hold);
            }
            AsrEvent::Endpoint => {
                self.finalize_pending = false;
                self.last_speech_endpoint_at = Some(Instant::now());
                self.log_session_event("SpeechEndpoint");
                let segment = self.transcript.take_segment();
                self.commit_segment(segment);
                if self.committed_hold_until.is_none() && self.overlay_error_until.is_none() {
                    self.hide_overlay();
                }
            }
            AsrEvent::FinalizeDone => {
                // endpoint_mode=client では <end> が来ないので、ここで区切り時刻を更新する。
                // 更新しないと last_speech_endpoint_at が発話開始のまま止まり、
                // idle_close_sec を過ぎた後は毎周期 finalize を送り続ける。
                self.finalize_pending = false;
                self.last_speech_endpoint_at = Some(Instant::now());
                self.log_session_event("FinalizeDone");
                let segment = self.transcript.take_segment();
                self.commit_segment(segment);
                if self.committed_hold_until.is_none() {
                    self.refresh_overlay();
                }
            }
            AsrEvent::Finished => {
                self.finalize_pending = false;
                self.asr_closing = true;
                self.log_session_event("Finished");
                let segment = self.transcript.take_segment();
                self.commit_segment(segment);
                if self.committed_hold_until.is_none() {
                    self.refresh_overlay();
                }
                if self.session.apply(SessionInput::Finished) {
                    let next_state = self.session.state();
                    self.send_ui(UiUpdate::State(next_state));
                    self.cleanup_asr();
                    self.resume_after_finished();
                    self.apply_pending_settings();
                }
            }
            AsrEvent::Failed(error) => {
                if is_nonfatal_asr_error(&error) {
                    if self.asr_closing || self.session.state() == SessionState::Closing {
                        tracing::debug!("ASR connection closed during normal shutdown: {error}");
                        self.finish_asr_shutdown();
                    } else {
                        self.abort_asr_session(error);
                    }
                } else {
                    tracing::error!("ASR session failed: {error}");
                    let message = asr_user_error_message(&error).to_string();
                    self.fail_runtime(message);
                }
            }
        }
    }

    fn finish_asr_shutdown(&mut self) {
        self.cleanup_asr();
        let _ = self.transcript.take_segment();
        if !self.session.apply(SessionInput::Finished) && !self.session.apply(SessionInput::Aborted)
        {
            return;
        }
        self.hide_overlay();
        self.send_ui(UiUpdate::State(self.session.state()));
        self.resume_after_finished();
        self.apply_pending_settings();
    }

    fn abort_asr_session(&mut self, error: AsrError) {
        tracing::warn!("ASR session ended normally: {error}");
        self.send_stop();
        self.cleanup_asr();
        let _ = self.transcript.take_segment();
        if !self.session.apply(SessionInput::Aborted) {
            return;
        }
        self.hide_overlay();
        self.send_ui(UiUpdate::State(SessionState::Listening));
        self.resume_after_finished();
        self.apply_pending_settings();
    }

    fn resume_after_finished(&mut self) {
        if self.session.state() != SessionState::Listening {
            return;
        }
        // 常時待受ではマイクを保持し続けるので、audio_capture の有無で
        // 条件分岐してはならない。ここを条件付きにすると VAD が再武装されず、
        // 起動後に発話を 1 回しか検知できなくなる。
        self.reset_vad_state();
        if self.audio_capture.is_none() {
            if let Err(error) = self.start_audio_capture() {
                self.suspend_vad();
                self.fail_runtime_user_action(format!("failed to resume microphone: {error:#}"));
            }
        }
    }

    fn periodic(&mut self) {
        self.text_out.poll_paste_target();
        self.log_audio_progress();
        self.prune_level_window(Instant::now());
        let status = level_status(
            self.level_peak,
            self.level_clip_window
                .iter()
                .map(|window| window.clipped)
                .sum(),
            self.level_clip_window
                .iter()
                .map(|window| window.total)
                .sum(),
        );
        self.send_ui(UiUpdate::Level {
            peak: self.level_peak,
            status,
        });
        self.level_peak = 0;

        self.check_session_timeouts();
        self.check_failed_recovery();
        self.check_splash_timeout();
        self.check_overlay_timeout();
        self.check_commit_hold_timeout();
        if self.session.state() != SessionState::Streaming {
            return;
        }

        let Some(last_speech_endpoint_at) = self.last_speech_endpoint_at else {
            return;
        };
        if last_speech_endpoint_at.elapsed()
            <= Duration::from_secs(self.settings.idle_close_sec as u64)
        {
            return;
        }

        if !self.send_finalize() {
            return;
        }
        self.send_stop();
        if self.session.apply(SessionInput::IdleTimeout) {
            self.log_session_event("IdleTimeout");
            self.last_speech_endpoint_at = None;
            self.closing_started_at = Some(Instant::now());
            self.hide_overlay();
            self.send_ui(UiUpdate::State(SessionState::Closing));
        }
    }

    fn check_session_timeouts(&mut self) {
        let now = Instant::now();
        match self.session.state() {
            SessionState::Closing
                if self
                    .closing_started_at
                    .is_some_and(|started| now.duration_since(started) >= CLOSING_TIMEOUT) =>
            {
                tracing::warn!("closing timed out without finished");
                self.reset_after_session_timeout();
            }
            SessionState::Connecting
                if self
                    .connecting_started_at
                    .is_some_and(|started| now.duration_since(started) >= CONNECTING_TIMEOUT) =>
            {
                tracing::warn!("connecting timed out");
                self.reset_after_session_timeout();
            }
            _ => {}
        }
    }

    fn check_failed_recovery(&mut self) {
        if !self.failed_recovery_enabled {
            return;
        }
        let Some(failed_at) = self.failed_at else {
            return;
        };
        let retry_delay = self.failed_retry_delay;
        if !failed_retry_is_due(failed_at, retry_delay, Instant::now()) {
            return;
        }

        tracing::warn!(
            "recovering from failed state after {}s",
            retry_delay.as_secs()
        );
        if !self.session.apply(SessionInput::Retry) {
            return;
        }
        self.failed_at = None;
        self.failed_recovery_enabled = false;
        self.failed_retry_delay = next_failed_retry_delay(retry_delay);
        self.hide_overlay();
        self.send_ui(UiUpdate::State(SessionState::Listening));
        tracing::info!("session recovered to Listening");
        self.resume_after_finished();
        self.apply_pending_settings();
    }

    fn reset_after_session_timeout(&mut self) {
        self.send_stop();
        self.cleanup_asr();
        self.hide_overlay();
        if !self.session.apply(SessionInput::Timeout) {
            return;
        }
        self.send_ui(UiUpdate::State(SessionState::Listening));
        self.reset_vad_state();
        if self.audio_capture.is_none() {
            if let Err(error) = self.start_audio_capture() {
                self.suspend_vad();
                self.fail_runtime_user_action(format!("failed to resume microphone: {error:#}"));
            }
        }
        self.apply_pending_settings();
    }

    fn commit_segment(&mut self, segment: Option<String>) {
        if let Some(segment) = segment {
            self.pending_commit.push_str(&segment);
        }

        if !self.settings.auto_paste {
            if let Some(segment) = take_pending(&mut self.pending_commit) {
                if self.accept_commit(&segment) {
                    self.show_committed_text(segment);
                }
            }
            return;
        }

        if !self.settings.paste_per_endpoint && self.session.state() != SessionState::Closing {
            return;
        }

        let Some(text) = take_pending(&mut self.pending_commit) else {
            return;
        };
        if !self.accept_commit(&text) {
            return;
        }
        self.show_committed_text(text.clone());
        if let Err(error) = self.text_out.emit(&text, PasteMethod::ClipboardAndPaste) {
            self.report_error(format!("failed to output transcript: {error:#}"));
        }
    }

    fn accept_commit(&mut self, text: &str) -> bool {
        let now = Instant::now();
        if self
            .last_commit
            .as_ref()
            .is_some_and(|(last_text, committed_at)| {
                last_text == text && now.duration_since(*committed_at) <= DUPLICATE_COMMIT_WINDOW
            })
        {
            tracing::warn!("duplicate commit dropped");
            return false;
        }
        self.last_commit = Some((text.to_string(), now));
        true
    }

    fn fail_runtime(&mut self, message: String) {
        self.fail_runtime_with_policy(message, true);
    }

    fn fail_runtime_user_action(&mut self, message: String) {
        self.fail_runtime_with_policy(message, false);
    }

    fn fail_runtime_with_policy(&mut self, message: String, auto_recover: bool) {
        let message = self.sanitize_message(message);
        tracing::error!("{message}");
        let entered_failed = self.session.apply(SessionInput::Failed);
        if entered_failed || self.session.state() == SessionState::Failed {
            if entered_failed {
                self.failed_at = Some(Instant::now());
            }
            self.failed_recovery_enabled = auto_recover;
            self.send_ui(UiUpdate::State(SessionState::Failed));
        }
        if auto_recover {
            self.show_overlay_error(message);
        } else {
            self.show_persistent_overlay_error(message);
        }
        self.send_stop();
        self.cleanup_asr();
    }

    fn report_error(&mut self, message: String) {
        let message = self.sanitize_message(message);
        tracing::error!("{message}");
        self.show_overlay_error(message);
    }

    fn sanitize_message(&self, message: String) -> String {
        if let Some(key) = &self.active_api_key {
            message.replace(key, "[redacted]")
        } else {
            message
        }
    }

    fn send_text_update(&mut self, force: bool) {
        let now = Instant::now();
        if !force
            && self
                .last_text_ui
                .is_some_and(|last| now.duration_since(last) < TEXT_UI_MIN_INTERVAL)
        {
            return;
        }
        self.last_text_ui = Some(now);
        self.refresh_overlay();
    }

    fn check_overlay_timeout(&mut self) {
        if self
            .overlay_error_until
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.hide_overlay();
        }
    }

    fn check_commit_hold_timeout(&mut self) {
        if self
            .committed_hold_until
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.hide_overlay();
        }
    }

    fn check_splash_timeout(&mut self) {
        if !matches!(self.overlay, OverlayView::Splash) {
            return;
        }
        if self.splash_started_at.is_none_or(|started| {
            started.elapsed() < Duration::from_millis(self.settings.splash_ms as u64)
        }) {
            return;
        }

        self.splash_started_at = None;
        if self.connection_needs_attention() {
            self.require_connection();
        } else {
            self.hide_overlay();
        }
    }

    fn show_overlay_error(&mut self, message: String) {
        self.splash_started_at = None;
        self.overlay_error_until = Some(Instant::now() + OVERLAY_ERROR_DURATION);
        self.set_overlay(OverlayView::Shown {
            kind: OverlayKind::Error,
            committed: String::new(),
            partial: String::new(),
            error: message,
        });
    }

    fn show_persistent_overlay_error(&mut self, message: String) {
        self.splash_started_at = None;
        self.overlay_error_until = None;
        self.set_overlay(OverlayView::Shown {
            kind: OverlayKind::Error,
            committed: String::new(),
            partial: String::new(),
            error: message,
        });
    }

    fn hide_overlay(&mut self) {
        self.overlay_error_until = None;
        self.splash_started_at = None;
        self.committed_hold_until = None;
        self.set_overlay(OverlayView::Hidden);
    }

    fn clear_commit_hold(&mut self) -> bool {
        self.committed_hold_until.take().is_some()
    }

    fn show_committed_text(&mut self, text: String) {
        if self.settings.commit_hold_ms == 0 {
            self.hide_overlay();
            return;
        }
        self.overlay_error_until = None;
        self.splash_started_at = None;
        self.committed_hold_until =
            Some(Instant::now() + Duration::from_millis(u64::from(self.settings.commit_hold_ms)));
        self.set_overlay(OverlayView::Shown {
            kind: OverlayKind::Committed,
            committed: text,
            partial: String::new(),
            error: String::new(),
        });
    }

    fn login_required(&self) -> bool {
        matches!(self.provider.readiness(), Readiness::NeedsLogin { .. })
    }

    fn connection_needs_attention(&self) -> bool {
        !matches!(self.provider.readiness(), Readiness::Ready)
    }

    fn refresh_overlay(&mut self) {
        if self.overlay_error_until.is_some() {
            return;
        }
        if self.committed_hold_until.is_some() {
            return;
        }
        let view = if self.session.state() == SessionState::Connecting {
            OverlayView::Shown {
                kind: OverlayKind::Connecting,
                committed: String::new(),
                partial: String::new(),
                error: String::new(),
            }
        } else if self.session.state() == SessionState::Streaming
            && (self.gate.is_speaking() || self.finalize_pending || !self.transcript.is_empty())
        {
            // 発話中は「音声入力中」、finalize の結果待ちは「認識中」。
            // 結果待ちを表示しないと、話し終わってから貼り付くまでの
            // 数百ミリ秒〜1 秒、窓が消えて止まったように見える。
            let kind = if self.gate.is_speaking() {
                OverlayKind::Recognizing
            } else if self.finalize_pending {
                OverlayKind::Finalizing
            } else {
                OverlayKind::Recognizing
            };
            OverlayView::Shown {
                kind,
                committed: self.transcript.committed().to_string(),
                partial: self.transcript.partial().to_string(),
                error: String::new(),
            }
        } else {
            OverlayView::Hidden
        };
        self.set_overlay(view);
    }

    fn set_overlay(&mut self, view: OverlayView) {
        if self.overlay == view {
            return;
        }
        self.overlay = view.clone();
        self.send_ui(UiUpdate::Overlay(view));
    }

    fn send_ui(&self, update: UiUpdate) {
        let _ = self.to_ui.try_send(update);
    }

    fn log_session_event(&self, event: &'static str) {
        let Some(session_started_at) = self.session_started_at else {
            return;
        };
        tracing::debug!(
            target: "otoa_input",
            event,
            elapsed_ms = session_started_at.elapsed().as_millis() as u64,
            "controller event"
        );
    }

    fn log_audio_progress(&mut self) {
        let Some(session_started_at) = self.session_started_at else {
            return;
        };
        let now = Instant::now();
        if self
            .last_audio_log_at
            .is_some_and(|last| now.duration_since(last) < AUDIO_PROGRESS_LOG_INTERVAL)
        {
            return;
        }
        self.last_audio_log_at = Some(now);
        let rms = rms_from_stats(self.sent_level_sum_squares, self.sent_level_samples);
        tracing::debug!(
            target: "otoa_input",
            sent_audio_ms = self.sent_audio_ms,
            state = ?self.session.state(),
            rms,
            peak = self.sent_level_peak,
            frames = self.sent_frames,
            elapsed_ms = session_started_at.elapsed().as_millis() as u64,
            "controller audio progress"
        );
        self.sent_level_sum_squares = 0;
        self.sent_level_peak = 0;
        self.sent_level_samples = 0;
        self.sent_frames = 0;
    }

    fn record_vad_level(&mut self, samples: &[i16], probs: &[f32]) {
        if let Ok(path) = std::env::var("OTOA_DUMP_AUDIO") {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let mut bytes = Vec::with_capacity(samples.len() * 2);
                for s in samples {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                let _ = f.write_all(&bytes);
            }
        }
        for &sample in samples {
            self.vad_level_sum_squares += u64::from(i32::from(sample).unsigned_abs().pow(2));
            self.vad_level_peak = self.vad_level_peak.max(sample_level(sample));
        }
        self.vad_level_samples += samples.len() as u64;
        self.vad_prob_max = self
            .vad_prob_max
            .max(probs.iter().copied().fold(0.0, f32::max));

        let now = Instant::now();
        if now.duration_since(self.last_vad_log_at) < AUDIO_PROGRESS_LOG_INTERVAL {
            return;
        }
        self.last_vad_log_at = now;
        tracing::debug!(
            target: "otoa_input",
            rms = rms_from_stats(self.vad_level_sum_squares, self.vad_level_samples),
            peak = self.vad_level_peak,
            "vad input"
        );
        tracing::debug!(target: "otoa_input", prob_max = self.vad_prob_max, "vad");
        self.vad_level_sum_squares = 0;
        self.vad_level_peak = 0;
        self.vad_level_samples = 0;
        self.vad_prob_max = 0.0;
    }

    fn record_level_window(&mut self, samples: &[i16]) {
        let now = Instant::now();
        let clipped = samples
            .iter()
            .filter(|&&sample| sample_level(sample) >= 32_000)
            .count() as u64;
        self.level_clip_window.push_back(LevelWindow {
            at: now,
            clipped,
            total: samples.len() as u64,
        });
        self.prune_level_window(now);
    }

    fn prune_level_window(&mut self, now: Instant) {
        while self
            .level_clip_window
            .front()
            .is_some_and(|window| now.duration_since(window.at) > Duration::from_secs(1))
        {
            self.level_clip_window.pop_front();
        }
    }

    /// 次の発話を検知できる状態へ戻す。
    ///
    /// プリロールは消さない。接続を閉じている間に話し始めていた場合、その
    /// 音声はプリロールにしか残っておらず、消すと先頭が欠ける。
    /// 古い音声はリングバッファの容量で自然に押し出される。
    fn reset_vad_state(&mut self) {
        self.gate.reset();
        let _ = self.vad_control.send(VadControl::Resume);
    }

    fn suspend_vad(&self) {
        let _ = self.vad_control.send(VadControl::Suspend);
    }

    fn rebuild_vad_configuration(&mut self) {
        self.gate = gate_from_settings(&self.settings);
        self.preroll = PreRoll::new(milliseconds_to_samples(self.settings.preroll_ms));
    }

    fn apply_pending_settings(&mut self) {
        let Some(settings) = self.pending_settings.take() else {
            return;
        };
        let was_enabled = self.settings.listening_enabled;
        self.settings = settings;
        self.rebuild_vad_configuration();
        if was_enabled && !self.settings.listening_enabled {
            self.disable_listening();
        }
    }

    fn request_shutdown(&mut self) {
        let was_closing = self.session.state() == SessionState::Closing;
        self.disable_listening();
        if was_closing {
            self.send_stop();
        }
        let _ = self.vad_control.send(VadControl::Shutdown);
    }

    fn cleanup_asr(&mut self) {
        self.finalize_pending = false;
        self.client_finalize_sent = false;
        self.to_asr = None;
        self.asr_events = None;
        if let Some(session_thread) = self.asr_thread.take() {
            let _ = session_thread.join();
        }
        self.pending_audio.clear();
        self.active_api_key = None;
        self.session_started_at = None;
        self.connecting_started_at = None;
        self.closing_started_at = None;
        self.last_speech_endpoint_at = None;
        self.sent_audio_ms = 0;
        self.sent_level_sum_squares = 0;
        self.sent_level_peak = 0;
        self.sent_level_samples = 0;
        self.sent_frames = 0;
        self.last_audio_log_at = None;
        self.asr_closing = false;
    }
}

fn next_failed_retry_delay(current: Duration) -> Duration {
    current
        .checked_mul(2)
        .unwrap_or(FAILED_RETRY_MAX)
        .min(FAILED_RETRY_MAX)
}

fn failed_retry_is_due(failed_at: Instant, retry_delay: Duration, now: Instant) -> bool {
    now.duration_since(failed_at) >= retry_delay
}

fn is_nonfatal_asr_error(error: &AsrError) -> bool {
    matches!(error, AsrError::Io(_) | AsrError::ClosedEarly)
}

fn asr_user_error_message(error: &AsrError) -> &'static str {
    match error {
        AsrError::Connect(_) | AsrError::Io(_) => "音声認識サービスへの接続に失敗しました",
        AsrError::Decode(_) => "音声認識サービスからの応答を処理できませんでした",
        AsrError::ClosedEarly => "音声認識セッションが予期せず終了しました",
        AsrError::Server { .. } => "音声認識サーバーでエラーが発生しました",
    }
}

fn is_user_action_failure_message(message: &str) -> bool {
    matches!(
        message,
        GATEWAY_URL_MISSING_MESSAGE
            | "サーバー URL を設定してください"
            | "音声認識の認証情報が設定されていません"
    )
}

fn gate_from_settings(settings: &Settings) -> SpeechGate {
    SpeechGate::new(
        settings.vad_threshold,
        milliseconds_to_frames(settings.vad_min_speech_ms),
        milliseconds_to_frames(settings.vad_min_silence_ms),
    )
}

fn short_login_error(error: &anyhow::Error) -> String {
    let message = error.to_string();
    let message = message.lines().next().unwrap_or("ログインに失敗しました");
    let mut short = message.chars().take(120).collect::<String>();
    if message.chars().count() > 120 {
        short.push('…');
    }
    short
}

fn milliseconds_to_frames(milliseconds: u32) -> usize {
    (u64::from(milliseconds).div_ceil(u64::from(VAD_FRAME_MS))).max(1) as usize
}

fn milliseconds_to_samples(milliseconds: u32) -> usize {
    ((milliseconds as u64 * VAD_SAMPLE_RATE as u64) / 1000) as usize
}

fn samples_to_bytes(samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

fn apply_input_gain<'a>(samples: &'a [i16], gain: f32) -> Cow<'a, [i16]> {
    let gain = if gain > 0.0 { gain } else { 1.0 };
    if gain == 1.0 {
        return Cow::Borrowed(samples);
    }

    Cow::Owned(
        samples
            .iter()
            .map(|&sample| ((sample as f32) * gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16)
            .collect(),
    )
}

fn rms_from_stats(sum_squares: u64, samples: u64) -> i16 {
    if samples == 0 {
        return 0;
    }
    ((sum_squares as f64 / samples as f64).sqrt().round() as i64).min(i16::MAX as i64) as i16
}

fn sample_level(sample: i16) -> i16 {
    sample.unsigned_abs().min(i16::MAX as u16) as i16
}

struct LevelWindow {
    at: Instant,
    clipped: u64,
    total: u64,
}

fn level_status(peak: i16, clipped_samples: u64, total_samples: u64) -> LevelStatus {
    if total_samples > 0 && clipped_samples.saturating_mul(1000) > total_samples {
        return LevelStatus::Clipped;
    }
    if f64::from(peak.unsigned_abs()) / f64::from(i16::MAX) < 0.03 {
        LevelStatus::TooQuiet
    } else {
        LevelStatus::Normal
    }
}

fn tokens_to_text(tokens: &[AsrToken]) -> String {
    tokens
        .iter()
        .map(|token| token.text.as_str())
        .collect::<String>()
}

fn take_pending(pending: &mut String) -> Option<String> {
    let text = std::mem::take(pending);
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_input_gain, failed_retry_is_due, is_nonfatal_asr_error, level_status,
        next_failed_retry_delay, Controller, LevelStatus, OverlayKind, OverlayView,
        FAILED_RETRY_INITIAL, FAILED_RETRY_MAX, GATEWAY_URL_MISSING_MESSAGE,
    };
    use crate::connection::SelfHostedProvider;
    use crate::settings::Settings;
    use otoa_input_core::{GateEvent, SessionInput, SessionState};
    use otoa_input_protocol::{AsrCommand, AsrError, AsrEvent, AsrToken};
    use std::time::{Duration, Instant};

    fn test_controller(settings: Settings) -> Controller {
        let (to_ui, _ui_rx) = crossbeam_channel::bounded(8);
        let (audio_sink, _audio_rx) = crossbeam_channel::bounded(8);
        let (vad_control, _vad_control_rx) = crossbeam_channel::bounded(8);
        let (_vad_event_tx, vad_events) = crossbeam_channel::bounded(8);
        let provider = std::sync::Arc::new(SelfHostedProvider);
        Controller::new(
            settings,
            provider,
            to_ui,
            audio_sink,
            vad_control,
            vad_events,
        )
        .expect("controller should initialize for the overlay test")
    }

    fn settings_with(update: impl FnOnce(&mut Settings)) -> Settings {
        let mut settings = Settings::default();
        update(&mut settings);
        settings
    }

    fn committed_overlay(controller: &Controller) -> (&str, &str) {
        let OverlayView::Shown {
            kind,
            committed,
            partial,
            ..
        } = &controller.overlay
        else {
            panic!("overlay should remain shown while holding a commit");
        };
        assert_eq!(*kind, OverlayKind::Committed);
        (committed, partial)
    }

    #[test]
    fn input_gain_doubles_amplitude() {
        assert_eq!(&*apply_input_gain(&[1000, -1000], 2.0), &[2000, -2000]);
    }

    #[test]
    fn input_gain_clamps_positive_overflow() {
        assert_eq!(
            &*apply_input_gain(&[20_000, -1_000], 2.0),
            &[i16::MAX, -2_000]
        );
    }

    #[test]
    fn input_gain_one_leaves_samples_unchanged() {
        let samples = [1234, -2345];
        let amplified = apply_input_gain(&samples, 1.0);
        assert!(matches!(amplified, std::borrow::Cow::Borrowed(_)));
        assert_eq!(&*amplified, &samples);
    }

    #[test]
    fn clip_warning_requires_more_than_point_one_percent() {
        assert_eq!(level_status(20_000, 2, 1_000), LevelStatus::Clipped);
        assert_eq!(level_status(20_000, 1, 1_000), LevelStatus::Normal);
    }

    #[test]
    fn normal_level_is_not_warning() {
        assert_eq!(level_status(16_384, 0, 1_000), LevelStatus::Normal);
    }

    #[test]
    fn quiet_level_is_warning_below_three_percent() {
        assert_eq!(level_status(983, 0, 1_000), LevelStatus::TooQuiet);
    }

    #[test]
    fn asr_io_failure_uses_nonfatal_listening_recovery() {
        let mut session = otoa_input_core::Session::new();
        assert!(session.apply(SessionInput::Enable));
        assert!(session.apply(SessionInput::SpeechStarted));
        assert!(session.apply(SessionInput::Connected));

        let error = AsrError::Io("peer closed without close_notify".to_string());
        assert!(is_nonfatal_asr_error(&error));
        assert!(session.apply(SessionInput::Aborted));
        assert_eq!(session.state(), SessionState::Listening);
    }

    #[test]
    fn failed_state_retries_after_its_delay() {
        let now = Instant::now();
        let failed_at = now - FAILED_RETRY_INITIAL;
        assert!(failed_retry_is_due(failed_at, FAILED_RETRY_INITIAL, now));

        let mut session = otoa_input_core::Session::new();
        assert!(session.apply(SessionInput::Enable));
        assert!(session.apply(SessionInput::Failed));
        assert!(session.apply(SessionInput::Retry));
        assert_eq!(session.state(), SessionState::Listening);
    }

    #[test]
    fn failed_retry_delay_backoff_is_capped() {
        let mut delay = FAILED_RETRY_INITIAL;
        let mut delays = Vec::new();
        for _ in 0..4 {
            delays.push(delay);
            delay = next_failed_retry_delay(delay);
        }
        assert_eq!(
            delays,
            [
                Duration::from_secs(5),
                Duration::from_secs(10),
                Duration::from_secs(20),
                FAILED_RETRY_MAX,
            ]
        );
        assert_eq!(next_failed_retry_delay(FAILED_RETRY_MAX), FAILED_RETRY_MAX);
    }

    #[test]
    fn user_action_failure_stays_failed_without_automatic_recovery() {
        let (to_ui, _ui_rx) = crossbeam_channel::bounded(8);
        let (audio_sink, _audio_rx) = crossbeam_channel::bounded(8);
        let (vad_control, _vad_control_rx) = crossbeam_channel::bounded(8);
        let (_vad_event_tx, vad_events) = crossbeam_channel::bounded(8);
        let settings = Settings::default();
        let provider = std::sync::Arc::new(SelfHostedProvider);
        let mut controller = Controller::new(
            settings,
            provider,
            to_ui,
            audio_sink,
            vad_control,
            vad_events,
        )
        .expect("controller should initialize for the policy test");

        controller.fail_runtime_user_action(GATEWAY_URL_MISSING_MESSAGE.to_string());

        assert_eq!(controller.session.state(), SessionState::Failed);
        assert!(!controller.failed_recovery_enabled);
        assert!(controller.failed_at.is_some());
        assert!(controller.overlay_error_until.is_none());
    }

    #[test]
    fn connected_resets_failed_retry_backoff() {
        let (to_ui, _ui_rx) = crossbeam_channel::bounded(8);
        let (audio_sink, _audio_rx) = crossbeam_channel::bounded(8);
        let (vad_control, _vad_control_rx) = crossbeam_channel::bounded(8);
        let (_vad_event_tx, vad_events) = crossbeam_channel::bounded(8);
        let settings = Settings::default();
        let provider = std::sync::Arc::new(SelfHostedProvider);
        let mut controller = Controller::new(
            settings,
            provider,
            to_ui,
            audio_sink,
            vad_control,
            vad_events,
        )
        .expect("controller should initialize for the reset test");
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        controller.failed_at = Some(Instant::now());
        controller.failed_recovery_enabled = true;
        controller.failed_retry_delay = Duration::from_secs(20);

        controller.handle_asr_event(otoa_input_protocol::AsrEvent::Connected);

        assert!(controller.failed_at.is_none());
        assert!(!controller.failed_recovery_enabled);
        assert_eq!(controller.failed_retry_delay, FAILED_RETRY_INITIAL);
    }

    #[test]
    fn committed_overlay_hides_after_hold_time() {
        let settings = settings_with(|settings| {
            settings.auto_paste = false;
            settings.commit_hold_ms = 800;
        });
        let mut controller = test_controller(settings);

        controller.commit_segment(Some("確定テキスト".to_string()));
        let (committed, partial) = committed_overlay(&controller);
        assert_eq!(committed, "確定テキスト");
        assert!(partial.is_empty());

        controller.committed_hold_until = Some(Instant::now() - Duration::from_millis(1));
        controller.check_commit_hold_timeout();
        assert_eq!(controller.overlay, OverlayView::Hidden);
    }

    #[test]
    fn next_speech_started_clears_committed_overlay_immediately() {
        let settings = settings_with(|settings| {
            settings.auto_paste = false;
            settings.commit_hold_ms = 800;
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let mut controller = test_controller(settings);
        controller.commit_segment(Some("前の発話".to_string()));
        assert!(controller.committed_hold_until.is_some());

        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        assert!(controller.session.apply(SessionInput::Connected));
        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);

        assert!(controller.committed_hold_until.is_none());
        assert_eq!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Recognizing,
                committed: String::new(),
                partial: String::new(),
                error: String::new(),
            }
        );
    }

    #[test]
    fn next_partial_result_replaces_committed_overlay_immediately() {
        let settings = settings_with(|settings| {
            settings.auto_paste = false;
            settings.commit_hold_ms = 800;
        });
        let mut controller = test_controller(settings);
        controller.commit_segment(Some("前の発話".to_string()));
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        assert!(controller.session.apply(SessionInput::Connected));

        controller.handle_asr_event(AsrEvent::PartialText(vec![AsrToken {
            text: "次の発話".to_string(),
            start_ms: None,
            end_ms: None,
            confidence: None,
            is_final: false,
            speaker: None,
            language: None,
            translation_status: None,
            source_language: None,
        }]));

        assert!(controller.committed_hold_until.is_none());
        assert_eq!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Recognizing,
                committed: String::new(),
                partial: "次の発話".to_string(),
                error: String::new(),
            }
        );
    }

    #[test]
    fn streaming_while_speaking_shows_empty_recognizing_overlay() {
        let settings = settings_with(|settings| {
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let mut controller = test_controller(settings);
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        assert!(controller.session.apply(SessionInput::Connected));
        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));

        controller.refresh_overlay();

        assert_eq!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Recognizing,
                committed: String::new(),
                partial: String::new(),
                error: String::new(),
            }
        );
    }

    #[test]
    fn streaming_without_speech_or_transcript_hides_overlay() {
        let mut controller = test_controller(Settings::default());
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        assert!(controller.session.apply(SessionInput::Connected));

        controller.refresh_overlay();

        assert_eq!(controller.overlay, OverlayView::Hidden);
    }

    #[test]
    fn streaming_without_speech_shows_nonempty_transcript() {
        let mut controller = test_controller(Settings::default());
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        assert!(controller.session.apply(SessionInput::Connected));
        controller.transcript.replace_partial("認識中");

        controller.refresh_overlay();

        assert_eq!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Recognizing,
                committed: String::new(),
                partial: "認識中".to_string(),
                error: String::new(),
            }
        );
    }

    #[test]
    fn speech_ended_in_server_mode_hides_empty_streaming_overlay() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let mut controller = test_controller(settings);
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        assert!(controller.session.apply(SessionInput::Connected));
        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.refresh_overlay();
        assert!(matches!(controller.overlay, OverlayView::Shown { .. }));

        assert_eq!(controller.gate.push(0.0), Some(GateEvent::SpeechEnded));
        controller.handle_gate_event(GateEvent::SpeechEnded);

        assert_eq!(controller.session.state(), SessionState::Streaming);
        assert_eq!(controller.overlay, OverlayView::Hidden);
    }

    #[test]
    fn speech_ended_in_client_mode_shows_finalizing_until_the_result_arrives() {
        // 話し終えてから確定が返るまでオーバーレイを隠すと、認識が止まった
        // ように見える。この区間は「認識中」を出し続ける。
        let settings = settings_with(|settings| {
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let mut controller = test_controller(settings);
        let (to_asr, asr_commands) = crossbeam_channel::unbounded();
        controller.to_asr = Some(to_asr);
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        assert!(controller.session.apply(SessionInput::Connected));
        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);

        assert_eq!(controller.gate.push(0.0), Some(GateEvent::SpeechEnded));
        controller.handle_gate_event(GateEvent::SpeechEnded);

        assert!(matches!(asr_commands.try_recv(), Ok(AsrCommand::Finalize)));
        assert_eq!(controller.session.state(), SessionState::Streaming);
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Finalizing,
                ..
            }
        ));

        controller.handle_asr_event(AsrEvent::FinalizeDone);
        assert!(!controller.finalize_pending);
    }

    #[test]
    fn speech_endpoint_hides_overlay_when_commit_hold_is_zero() {
        let settings = settings_with(|settings| {
            settings.auto_paste = false;
            settings.commit_hold_ms = 0;
        });
        let mut controller = test_controller(settings);
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        assert!(controller.session.apply(SessionInput::Connected));
        controller.transcript.push_final("確定テキスト");
        controller.refresh_overlay();

        controller.handle_asr_event(AsrEvent::Endpoint);

        assert_eq!(controller.overlay, OverlayView::Hidden);
    }

    #[test]
    fn zero_commit_hold_hides_overlay_immediately() {
        let settings = settings_with(|settings| {
            settings.auto_paste = false;
            settings.commit_hold_ms = 0;
        });
        let mut controller = test_controller(settings);

        controller.commit_segment(Some("即時非表示".to_string()));

        assert_eq!(controller.overlay, OverlayView::Hidden);
        assert!(controller.committed_hold_until.is_none());
    }
}
