use crate::bundled_server;
use crate::settings::Settings;
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use otoa_input_core::Account;
use otoa_input_core::{
    ConnectionProvider, EnrollOutcome, EnrollReason, GateEvent, PasteShortcutSetting, PreRoll,
    Readiness, Session, SessionInput, SessionState, SpeechGate, Transcript,
};
use otoa_input_platform::{AudioCapture, AudioFrame, PasteMethod, PasteShortcut, TextOutput};
use otoa_input_protocol::{
    AsrCommand, AsrConfig, AsrError, AsrEvent, AsrSession, AsrToken, POLICY_VIOLATION_CLOSE_CODE,
};
use otoa_input_vad::{VAD_FRAME_MS, VAD_SAMPLE_RATE};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

const PENDING_AUDIO_LIMIT: usize = 100;
const CONTROLLER_TICK: Duration = Duration::from_millis(100);
const TEXT_UI_MIN_INTERVAL: Duration = Duration::from_millis(30);
const AUDIO_PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(1);
const DUPLICATE_COMMIT_WINDOW: Duration = Duration::from_secs(5);
const OVERLAY_ERROR_DURATION: Duration = Duration::from_secs(8);
const OVERLAY_NOTICE_DURATION: Duration = Duration::from_secs(3);
/// 通常応答の実機中央値は 2.58 秒。通常時に待機表示を出さない余裕として 4.0 秒にする。
const SERVER_RESPONSE_WAITING_OVERLAY_DELAY: Duration = Duration::from_secs(4);
/// 接続処理そのものが長引いたときだけ、サーバー起動待ちとして見せる。
const CONNECTING_STARTING_OVERLAY_DELAY: Duration = Duration::from_secs(4);
/// コールドスタートは実測 16〜64 秒なので、最初の応答には十分な猶予を持たせる。
const SERVER_FIRST_RESPONSE_TIMEOUT: Duration = Duration::from_secs(75);
/// 途中結果が止まった後は、接続を無期限に保持しない。
const SERVER_FINAL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
const FAILED_RETRY_INITIAL: Duration = Duration::from_secs(5);
const FAILED_RETRY_MAX: Duration = Duration::from_secs(30);
/// enrollment のリモート失敗は、100 ms tick ごとに再試行せず最低 5 秒待つ。
const ENROLL_RETRY_INITIAL: Duration = Duration::from_secs(5);
const ENROLL_RETRY_MAX: Duration = Duration::from_secs(60);
/// Modal の scaledown_window と揃える。これ以上 ASR の成功応答が無ければ、
/// 次の発話の前に enrollment を送り、認識器が起きるまで待つ。
const WARMUP_IDLE_THRESHOLD: Duration = Duration::from_secs(60);
/// 最後に喋ってからこれだけ経ったら、暖機をやめる。
///
/// **止めないと、開いているだけで GPU が一日中起きたままになる。**
/// 向こうは使った時間で課金されるので、使っていないなら 0 台に落とす意味がある。
/// 落ちた後の 1 発話はコールドスタートを待つが、それは「久しぶりに使う」ときだけで、
/// 会話の合間の 60 秒はこの窓の内側なので待たない。
const WARMUP_ACTIVE_WINDOW: Duration = Duration::from_secs(10 * 60);
#[allow(dead_code)]
const CONNECTING_TIMEOUT: Duration = Duration::from_secs(10);
#[allow(dead_code)]
const CLOSING_TIMEOUT: Duration = Duration::from_secs(8);
/// warmup を待っているあいだに溜めた発話。
///
/// **warmup が成功しても捨てない。** 音声はここに揃っているので、捨てる理由が
/// 無い。以前は成功したときだけ捨て、失敗したときだけ送っていた。利用者から
/// 見ると「たまに何も起きない」になり、warmup の窓は 1 秒未満で消えるので
/// 何が起きたのかも分からなかった。
#[derive(Default)]
struct DeferredSpeech {
    /// 喋り出しの直前まで溜めてあった音声。
    ///
    /// **捨ててはいけない。** warmup は 250ms 程度で終わるので、保留のあいだに
    /// 溜まるのは 1 フレームほどしかない。頭を落とすと、送られるのは語尾だけの
    /// 短い音声になり、話者照合のチャンクが足りずに「登録した声と一致しません
    /// でした」で弾かれる。**実際にそうなった。**
    preroll: Vec<i16>,
    audio: Vec<Vec<i16>>,
    /// VAD が終話まで見届けたか。
    ended: bool,
}

const GATEWAY_URL_MISSING_MESSAGE: &str =
    "ゲートウェイURLが設定されていません。設定画面の「詳細」で指定してください。";

fn configure_text_output(text_out: &mut TextOutput, settings: &Settings) {
    text_out.set_paste_shortcut(resolve_paste_shortcut(settings.paste_shortcut));
    text_out.set_restore_primary_selection(settings.restore_primary_selection);
}

fn resolve_paste_shortcut(setting: PasteShortcutSetting) -> PasteShortcut {
    match setting {
        PasteShortcutSetting::Auto | PasteShortcutSetting::ShiftInsert => {
            PasteShortcut::ShiftInsert
        }
        PasteShortcutSetting::CtrlV => PasteShortcut::CtrlV,
        PasteShortcutSetting::CtrlShiftV => PasteShortcut::CtrlShiftV,
    }
}

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
    /// 認識モデルを自動ダウンロード中。進捗の文言は `committed` に載せる。
    Preparing,
    /// 認識器を起こし、参照音声を登録している。発話は受け付けない。
    WarmingUp,
    Connecting,
    /// マイクが今の発話を拾っている。
    Recognizing,
    /// 発話は終わり、`finalize` の結果を待っている。
    /// この状態を持たないと、認識待ちの間だけオーバーレイが消えて、
    /// 何も起きていないように見える。
    Finalizing,
    /// 端末の VAD が無音を検知したあと、サーバーの確定応答を待っている。
    WaitingForResponse,
    /// その応答待ちが長く、サーバーの起動を待っている。
    StartingServer,
    Committed,
    /// セッションを継続したまま短時間だけ表示する通知。
    Notice,
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
    Route { local: bool },
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

/// VAD が今の発話を拾っているか。表示を導出するためだけの状態で、
/// 実際の VAD 制御は [`SpeechGate`] が引き続き所有する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateState {
    Idle,
    Speaking,
}

/// 一つの server turn で観測済みの応答相。
///
/// `Completed` は server endpoint が端末 VAD の `SpeechEnded` より先に来た事実を
/// bool を足さずに保持するために必要である。次の `SpeechStarted` だけが `Idle` へ戻す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerTurn {
    Idle,
    AwaitingFirstResponse { since: Instant },
    Receiving { last_activity_at: Instant },
    Completed,
}

impl ServerTurn {
    fn blocks_idle_close(self, now: Instant) -> bool {
        match self {
            Self::AwaitingFirstResponse { since } => {
                now.saturating_duration_since(since) < SERVER_FIRST_RESPONSE_TIMEOUT
            }
            Self::Receiving { last_activity_at } => {
                now.saturating_duration_since(last_activity_at) < SERVER_FINAL_RESPONSE_TIMEOUT
            }
            Self::Idle | Self::Completed => false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ServerActivity {
    Response,
    Completed,
    Ended,
}

#[derive(Debug, Clone)]
enum NoticeKind {
    Asr,
}

#[derive(Debug, Clone)]
struct Notice {
    kind: NoticeKind,
    message: String,
    until: Instant,
}

#[derive(Debug, Clone)]
enum UserError {
    Temporary { message: String, until: Instant },
    Persistent { kind: OverlayKind, message: String },
}

/// オーバーレイを導出するための事実。
///
/// `overlay` 自体は描画済みの前回値だけを保持する。何を表示するかは必ず
/// この値から [`view`] で求める。
#[derive(Debug, Clone)]
struct Facts {
    session: SessionState,
    gate: GateState,
    warmup: Option<Instant>,
    commit: Option<(String, Instant)>,
    notice: Option<Notice>,
    error: Option<UserError>,
    /// 途中結果は gate が Idle になった後にも届くため、gate と独立に持つ。
    partial: Option<String>,
    /// client endpoint の finalize を送ってから確定応答を受けるまで。
    finalizing: bool,
}

/// `Facts` と session に属する時刻・turn からオーバーレイを導出する。副作用を持たない。
///
fn view(
    facts: &Facts,
    server_turn: ServerTurn,
    connecting_started_at: Option<Instant>,
    now: Instant,
) -> OverlayView {
    match facts.error.as_ref() {
        Some(UserError::Temporary { message, until }) if now < *until => {
            return OverlayView::Shown {
                kind: OverlayKind::Error,
                committed: String::new(),
                partial: String::new(),
                error: message.clone(),
            };
        }
        Some(UserError::Persistent { kind, message }) => {
            return OverlayView::Shown {
                kind: *kind,
                committed: String::new(),
                partial: String::new(),
                error: message.clone(),
            };
        }
        Some(UserError::Temporary { .. }) | None => {}
    }

    if let Some(notice) = facts.notice.as_ref().filter(|notice| now < notice.until) {
        let _ = &notice.kind;
        return OverlayView::Shown {
            kind: OverlayKind::Notice,
            committed: String::new(),
            partial: String::new(),
            error: notice.message.clone(),
        };
    }

    if let Some((committed, _until)) = facts.commit.as_ref().filter(|(_, until)| now < *until) {
        return OverlayView::Shown {
            kind: OverlayKind::Committed,
            committed: committed.clone(),
            partial: String::new(),
            error: String::new(),
        };
    }

    if facts.warmup.is_some() {
        return blank_overlay(OverlayKind::WarmingUp);
    }

    if facts.session == SessionState::Connecting
        && connecting_started_at.is_some_and(|started_at| {
            now.saturating_duration_since(started_at) >= CONNECTING_STARTING_OVERLAY_DELAY
        })
    {
        return blank_overlay(OverlayKind::StartingServer);
    }

    if matches!(
        server_turn,
        ServerTurn::AwaitingFirstResponse { since }
            if now.saturating_duration_since(since) >= SERVER_RESPONSE_WAITING_OVERLAY_DELAY
    ) {
        return blank_overlay(OverlayKind::WaitingForResponse);
    }

    if let Some(partial) = facts.partial.as_ref().filter(|partial| !partial.is_empty()) {
        return OverlayView::Shown {
            kind: OverlayKind::Recognizing,
            committed: String::new(),
            partial: partial.clone(),
            error: String::new(),
        };
    }

    if facts.finalizing {
        return blank_overlay(OverlayKind::Finalizing);
    }

    if matches!(server_turn, ServerTurn::Receiving { .. }) {
        return blank_overlay(OverlayKind::Recognizing);
    }

    if facts.gate == GateState::Speaking {
        return blank_overlay(OverlayKind::Recognizing);
    }

    OverlayView::Hidden
}

fn blank_overlay(kind: OverlayKind) -> OverlayView {
    OverlayView::Shown {
        kind,
        committed: String::new(),
        partial: String::new(),
        error: String::new(),
    }
}

#[derive(Clone, Copy, Debug)]
enum WarmupReason {
    Startup,
    Idle,
    /// 設定で接続先が変わった直後。
    ///
    /// **切り替えて保存したら、その場で暖める。** 暖機は登録のときに打つので、
    /// それまで一度も使っていなかった接続先は冷えたままになる。次の待機タイマー
    /// か最初の発話まで気づけず、利用者は切り替えた直後だけ長く待たされる。
    SettingsChanged,
    /// **参照音声を録り直したら、その場で登録し直す。**
    ///
    /// 録り直しは設定を変えないので、接続先が変わったことにはならない。
    /// 次の無操作まで待つと、そのあいだサーバーは古い声で照合し続ける
    /// （実測で 5 分近く空いた）。
    VoiceChanged,
}

impl WarmupReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Idle => "idle",
            Self::SettingsChanged => "settings_changed",
            Self::VoiceChanged => "voice_changed",
        }
    }
}

#[cfg(test)]
struct WarmupResult {
    reason: WarmupReason,
    started_at: Instant,
    result: anyhow::Result<()>,
}

/// worker から返す、型付き enrollment 結果。`WarmupResult` は既存 unit test の
/// 局所的な失敗注入用として残し、実行経路は必ずこちらを使う。
struct EnrollmentWarmupResult {
    reason: WarmupReason,
    started_at: Instant,
    outcome: EnrollOutcome,
}

pub enum ControllerCommand {
    StartStop,
    StartLogin,
    Logout,
    UpdateSettings(Box<Settings>),
    /// 参照音声を録り直した。**その場で登録し直す。**
    ///
    /// 登録の道は暖機 1 本にまとめてある。呼ぶ側が自分で資格情報を取り直して
    /// 登録すると、同じことをする道が 2 つになり、片方だけ画面に出ない。
    RefreshVoiceEnrollment,
    Shutdown,
}

/// provider の判断を優先し、provider が接続可能と判断した場合に限って
/// 同梱サーバー由来の問題を表示用 readiness として加える。
fn combine_readiness(
    provider_readiness: Readiness,
    bundled_server_failure: Option<&str>,
) -> Readiness {
    match (provider_readiness, bundled_server_failure) {
        (Readiness::Ready, Some(message)) => Readiness::NeedsSetup {
            message: message.to_string(),
        },
        (readiness, _) => readiness,
    }
}

pub struct Controller {
    pub(crate) session: Session,
    pub(crate) transcript: Transcript,
    pub(crate) settings: Settings,
    pub(crate) pending_audio: Vec<Vec<u8>>,
    /// 接続待ちの上限で捨てた音声フレーム数。無音のまま失われないようログにも出す。
    pending_audio_dropped_frames: u64,
    pub(crate) to_asr: Option<Sender<AsrCommand>>,
    pub(crate) to_ui: Sender<UiUpdate>,
    pub(crate) text_out: TextOutput,
    facts: Facts,
    /// server endpoint mode の一つの論理 turn。応答待ちの時刻もこの相が所有する。
    server_turn: ServerTurn,
    /// 前回描画した値。表示理由は保持せず、Facts から導出した結果の重複送信を抑える。
    overlay: OverlayView,
    /// 第1・第2段のテストが直接観測している期限。製品の表示状態には使わない。
    #[cfg(test)]
    overlay_error_until: Option<Instant>,
    #[cfg(test)]
    overlay_notice_until: Option<Instant>,
    splash_started_at: Option<Instant>,
    /// enrollment worker が走っているか。
    warmup_in_progress: bool,
    warmup_result_rx: Option<Receiver<(u64, EnrollmentWarmupResult)>>,
    warmup_thread: Option<thread::JoinHandle<()>>,
    warmup_started_at: Option<Instant>,
    warmup_reason: Option<WarmupReason>,
    /// cancel 後に遅れて届いた worker 結果を捨てる世代。
    warmup_epoch: u64,
    /// `RetryableRemote` の次回試行時刻と指数バックオフ。
    warmup_retry_at: Option<Instant>,
    warmup_retry_delay: Duration,
    /// 最後に成功した ASR サービス応答。初回は warmup の成功を基準にする。
    last_successful_asr_response_at: Option<Instant>,
    /// warmup と競合した発話。**捨てずに、warmup が終わったら送る。**
    ///
    /// 3 つの変数（保留中か・音声・終話したか）に分けていたときは、
    /// 4 か所でばらばらに書き換えていて、成功したときだけ音声を捨てる分岐が
    /// できていた。持ち主を 1 つにして、消すか送るかを [`DeferredSpeech`] の
    /// 有無だけで決める。
    deferred_speech: Option<DeferredSpeech>,
    /// 保留していた発話で ASR を開始したとき、Connected になったら端末側で
    /// 終話を確定する（endpoint_mode=client のときだけ）。
    finish_after_deferred_warmup_connect: bool,
    /// 待受に入ってから 1 回は登録したか。**起動のたびに 1 回は必ず通す。**
    warmed_since_listening: bool,
    /// この発話は暖機に待たされたので、**貼り付けずクリップボードに置くだけ**にする。
    ///
    /// 暖機と接続で数秒かかることがあり、その間に利用者は別の窓へ移っている。
    /// そこへ貼ると、貼りたくない場所へ文字が入る。取り消せないので、結果が
    /// 遅れたときは自分で貼ってもらう。
    hold_paste_after_warmup: bool,
    /// 保留した発話で始めた接続が、まだ最初の応答を返していない。
    ///
    /// **待たせているあいだは、ひと続きの「準備中」として見せる。** 暖機が
    /// 終わった瞬間に表示を落とすと、実際はまだ待たせているのに「認識中」に
    /// 変わり、数秒後にまた「サーバーを起動しています」に変わる。1 つの状態が
    /// 3 つの文言を行き来して見える。
    warmup_until_response: Option<Instant>,
    /// サーバーが名乗った「認識器が寝るまでの時間」。
    /// 届いていなければ [`WARMUP_IDLE_THRESHOLD`] を使う。
    server_warmup_after: Option<Duration>,
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
    /// これが無いと SpeechEnded のたびに finalize を送り、同じ貼り付けを繰り返す。
    client_finalize_sent: bool,
    /// サーバーが `<end>` を返し、かつ端末側でも発話中でない間は、無音を
    /// 同じストリームへ送り続けない。次の発話に備えてプリロールだけ保つ。
    server_audio_paused: bool,
    /// `<end>` の時点で VAD がまだ喋っていたため止め損ねた送信停止。VAD が
    /// 黙った時点で適用する。これを持ち越さないと、以後 VAD と無関係に
    /// マイク入力が流れ続け、背景音が新しい発話として書き起こされる。
    pause_when_gate_stops: bool,
    /// `finalize` を送ってから結果が返るまで。オーバーレイの「文字にしています」表示に使う。
    finalize_pending: bool,
    /// 最後に「確実に声だった」フレームの時刻(確率 >= 0.9)。
    ///
    /// 利用者が体感する遅延の起点はここである。`vad edge`(閾値 0.5 を割った瞬間)
    /// ではない。閾値を割るのは声が消えかけてからで、体感より後になる。
    /// 2026-08-25、この起点で測らなかったために報告値(2.0 秒)と体感(4 秒)が
    /// 食い違い続けた。
    last_confident_speech_at: Option<Instant>,
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
    vad_frame_voiced: bool,
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
    #[cfg(test)]
    committed_hold_until: Option<Instant>,
    last_commit: Option<(String, Instant)>,
    last_text_ui: Option<Instant>,
    audio_capture: Option<AudioCapture>,
    pending_settings: Option<Settings>,
    /// 保留中の設定が効いたら暖機を打つか。接続先が変わったときだけ立てる。
    warmup_after_pending_settings: bool,
    /// このセッションで名乗られるべき方法。`None` なら確認しない。
    ///
    /// **確認できるまで転写を受け取らない。** 指定を知らないサーバーは、
    /// その指定を黙って捨てて別の経路で処理できてしまう。
    expected_backend: Option<String>,
    /// サーバーがその方法を名乗ったか。
    backend_confirmed: bool,
    active_api_key: Option<String>,
    provider: Arc<dyn ConnectionProvider>,
    /// provider とは別に保持する、同梱サーバーの起動失敗。
    /// provider 自身の readiness が Ready のときだけ表示用に合成する。
    bundled_server_failure: Option<String>,
    login_cancel: Option<Arc<AtomicBool>>,
    login_result_rx: Option<Receiver<anyhow::Result<()>>>,
    login_thread: Option<thread::JoinHandle<()>>,
}

impl Controller {
    pub fn new(
        settings: Settings,
        provider: Arc<dyn ConnectionProvider>,
        bundled_server_failure: Option<String>,
        to_ui: Sender<UiUpdate>,
        audio_sink: Sender<AudioFrame>,
        vad_control: Sender<VadControl>,
        vad_events: Receiver<VadMessage>,
    ) -> anyhow::Result<Self> {
        let gate = gate_from_settings(&settings);
        let preroll = PreRoll::new(milliseconds_to_samples(settings.preroll_ms));
        let splash_started_at = Instant::now();
        let mut text_out = TextOutput::new()?;
        configure_text_output(&mut text_out, &settings);
        Ok(Self {
            session: Session::new(),
            transcript: Transcript::new(),
            settings,
            pending_audio: Vec::new(),
            pending_audio_dropped_frames: 0,
            to_asr: None,
            to_ui,
            text_out,
            facts: Facts {
                session: SessionState::Disabled,
                gate: GateState::Idle,
                warmup: None,
                commit: None,
                notice: None,
                error: None,
                partial: None,
                finalizing: false,
            },
            server_turn: ServerTurn::Idle,
            // 起動直後はロゴを見せる。main の既定に合わせる。
            overlay: OverlayView::Splash,
            #[cfg(test)]
            overlay_error_until: None,
            #[cfg(test)]
            overlay_notice_until: None,
            splash_started_at: Some(splash_started_at),
            warmup_in_progress: false,
            warmup_result_rx: None,
            warmup_thread: None,
            warmup_started_at: None,
            warmup_reason: None,
            warmup_epoch: 0,
            warmup_retry_at: None,
            warmup_retry_delay: ENROLL_RETRY_INITIAL,
            last_successful_asr_response_at: None,
            deferred_speech: None,
            finish_after_deferred_warmup_connect: false,
            warmed_since_listening: false,
            hold_paste_after_warmup: false,
            warmup_until_response: None,
            server_warmup_after: None,
            gate,
            preroll,
            session_started_at: None,
            connecting_started_at: None,
            closing_started_at: None,
            last_speech_endpoint_at: None,
            sent_audio_ms: 0,
            asr_closing: false,
            client_finalize_sent: false,
            server_audio_paused: false,
            pause_when_gate_stops: false,
            finalize_pending: false,
            last_confident_speech_at: None,
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
            vad_frame_voiced: false,
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
            #[cfg(test)]
            committed_hold_until: None,
            last_commit: None,
            last_text_ui: None,
            audio_capture: None,
            pending_settings: None,
            warmup_after_pending_settings: false,
            expected_backend: None,
            // 接続のたびに Connected で決め直す。**それまでは確認済みとして扱う。**
            // 既定を未確認にすると、確認する必要のない構成でも文字を捨ててしまう。
            backend_confirmed: true,
            active_api_key: None,
            provider,
            bundled_server_failure,
            login_cancel: None,
            login_result_rx: None,
            login_thread: None,
        })
    }

    pub fn run(mut self, commands: Receiver<ControllerCommand>) {
        self.send_ui(UiUpdate::State(self.session.state()));
        self.send_account_update();
        self.send_login_state();
        self.send_route_update();
        self.render_overlay();
        // 同梱サーバーを立てる。モデルが無ければ自動で落とすので、ここで
        // 数分かかることがある。進捗はオーバーレイに出す。
        self.bootstrap_bundled_server();
        // **貼り付けの権限は起動時に確かめる。** ここで聞いておかないと、
        // 最初に喋った瞬間に初めて許可を求められることになる。
        if self.settings.auto_paste {
            if let Some(reason) = self.text_out.check_paste_permission() {
                tracing::warn!(reason = %reason, "貼り付けの権限がない");
                self.show_overlay_error(reason);
            }
        }
        if self.settings.listening_enabled && !self.connection_needs_attention() {
            self.enable_listening();
        } else {
            self.suspend_vad();
        }

        let ticker = crossbeam_channel::tick(CONTROLLER_TICK);
        let mut shutting_down = false;
        while !shutting_down {
            self.drain_warmup_events();
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
                        Ok(ControllerCommand::RefreshVoiceEnrollment) => {
                            let _ = self.start_warmup(WarmupReason::VoiceChanged);
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
                self.clear_overlay_facts();
                if self.settings.listening_enabled && self.session.state() == SessionState::Disabled
                {
                    self.enable_listening();
                }
                self.refresh_overlay();
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

    /// enrollment worker を開始する。実際に呼ぶ経路は `Listening` 中の idle
    /// scheduler と SpeechStarted の補助経路だけで、Disabled 中には走らない。
    fn start_warmup(&mut self, reason: WarmupReason) -> bool {
        if self.warmup_in_progress {
            return true;
        }
        if !self.provider.enrollment_is_eligible(&self.settings.core)
            || self
                .warmup_retry_at
                .is_some_and(|retry_at| Instant::now() < retry_at)
            || matches!(self.facts.error, Some(UserError::Persistent { .. }))
        {
            return false;
        }

        let started_at = Instant::now();
        let (result_tx, result_rx) = crossbeam_channel::bounded(1);
        let provider = Arc::clone(&self.provider);
        let settings = self.settings.core.clone();
        self.warmup_epoch = self.warmup_epoch.wrapping_add(1);
        let worker_epoch = self.warmup_epoch;

        self.warmup_in_progress = true;
        self.warmup_started_at = Some(started_at);
        self.warmup_reason = Some(reason);
        self.facts.warmup = Some(started_at);
        self.splash_started_at = None;
        self.refresh_overlay();
        tracing::info!(
            target: "otoa_input",
            epoch = worker_epoch,
            reason = reason.as_str(),
            "warmup: started"
        );

        let worker = thread::Builder::new()
            .name("otoa-warmup".to_string())
            .spawn(move || {
                let outcome = provider.ensure_enrolled(&settings, EnrollReason::Warmup);
                let _ = result_tx.send((
                    worker_epoch,
                    EnrollmentWarmupResult {
                        reason,
                        started_at,
                        outcome,
                    },
                ));
            });

        match worker {
            Ok(worker) => {
                self.warmup_result_rx = Some(result_rx);
                self.warmup_thread = Some(worker);
            }
            Err(error) => {
                self.finish_warmup_worker();
                self.finish_enrollment_warmup(
                    worker_epoch,
                    EnrollmentWarmupResult {
                        reason,
                        started_at,
                        outcome: EnrollOutcome::RetryableRemote(format!(
                            "起動処理を開始できませんでした: {error}"
                        )),
                    },
                );
            }
        }
        true
    }

    fn drain_warmup_events(&mut self) {
        let result = match self.warmup_result_rx.as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => Some(result),
            Some(Err(TryRecvError::Empty)) => None,
            Some(Err(TryRecvError::Disconnected)) => Some((
                self.warmup_epoch,
                EnrollmentWarmupResult {
                    reason: self.warmup_reason.unwrap_or(WarmupReason::Startup),
                    started_at: self.warmup_started_at.unwrap_or_else(Instant::now),
                    outcome: EnrollOutcome::RetryableRemote(
                        "起動処理が予期せず終了しました".to_string(),
                    ),
                },
            )),
            None => None,
        };
        let Some((worker_epoch, result)) = result else {
            return;
        };
        self.finish_warmup_worker();
        self.finish_enrollment_warmup(worker_epoch, result);
    }

    fn finish_warmup_worker(&mut self) {
        self.warmup_result_rx = None;
        self.warmup_started_at = None;
        self.warmup_reason = None;
        if let Some(worker) = self.warmup_thread.take() {
            let _ = worker.join();
        }
    }

    fn cancel_warmup(&mut self) {
        // reqwest の blocking request は途中で安全に取り消せない。終了を待つと
        // 最大 timeout までアプリを閉じられなくなるため、JoinHandle を外して
        // プロセス終了に任せる。世代を進め、遅れて届く結果も無視する。
        self.warmup_epoch = self.warmup_epoch.wrapping_add(1);
        self.warmup_in_progress = false;
        self.warmup_result_rx = None;
        self.warmup_started_at = None;
        self.warmup_reason = None;
        self.facts.warmup = None;
        self.clear_deferred_warmup_speech();
        self.warmup_thread.take();
    }

    fn finish_enrollment_warmup(&mut self, worker_epoch: u64, result: EnrollmentWarmupResult) {
        if worker_epoch != self.warmup_epoch || !self.warmup_in_progress {
            tracing::debug!(
                target: "otoa_input",
                worker_epoch,
                current_epoch = self.warmup_epoch,
                "discarded stale warmup result"
            );
            return;
        }

        let was_warming = self.is_warming_overlay();
        self.warmup_in_progress = false;
        self.facts.warmup = None;
        let elapsed_ms = result.started_at.elapsed().as_millis() as u64;
        match result.outcome {
            EnrollOutcome::Ready => {
                self.last_successful_asr_response_at = Some(Instant::now());
                self.warmup_retry_at = None;
                self.warmup_retry_delay = ENROLL_RETRY_INITIAL;
                tracing::info!(
                    target: "otoa_input",
                    reason = result.reason.as_str(),
                    elapsed_ms,
                    "warmup: done"
                );
                // **成功しても、待たせた発話は送る。** 音声は手元に揃って
                // いるのに、以前はここで捨てていた。利用者から見ると、喋った
                // のに何も起きない。warmup は 1 秒未満で終わるので、待たされた
                // ことにも気づけない。
                if self.deferred_speech.is_some() {
                    self.resume_deferred_warmup_speech();
                } else if was_warming {
                    self.refresh_overlay();
                }
            }
            EnrollOutcome::RetryableRemote(message) => {
                let retry_delay = self.warmup_retry_delay;
                self.warmup_retry_at = Some(Instant::now() + retry_delay);
                self.warmup_retry_delay = next_enroll_retry_delay(retry_delay);
                tracing::warn!(
                    target: "otoa_input",
                    reason = result.reason.as_str(),
                    elapsed_ms,
                    retry_after_secs = retry_delay.as_secs(),
                    error = %message,
                    "warmup: retryable remote failure"
                );
                if self.deferred_speech.is_some() {
                    // gateway は保存済み参照音声から回復できる。通信失敗で既に
                    // 始まった発話まで捨てず、保存した音声で接続を続ける。
                    self.resume_deferred_warmup_speech();
                } else if was_warming {
                    self.refresh_overlay();
                }
            }
            EnrollOutcome::NeedsUserAction(message) => {
                tracing::warn!(
                    target: "otoa_input",
                    reason = result.reason.as_str(),
                    elapsed_ms,
                    error = %message,
                    "warmup: user action required"
                );
                self.warmup_retry_at = None;
                self.clear_deferred_warmup_speech();
                self.fail_runtime_user_action(message);
            }
        }
    }

    /// 既存 unit test の局所的な `anyhow::Result` 注入用。実行中の worker は必ず
    /// `EnrollmentWarmupResult` を通るため、RetryableRemote の扱いには使わない。
    #[cfg(test)]
    fn finish_warmup(&mut self, result: WarmupResult) {
        let was_warming = self.is_warming_overlay();
        self.warmup_in_progress = false;
        self.facts.warmup = None;
        let elapsed_ms = result.started_at.elapsed().as_millis() as u64;
        match result.result {
            Ok(()) => {
                self.last_successful_asr_response_at = Some(Instant::now());
                tracing::info!(
                    target: "otoa_input",
                    reason = result.reason.as_str(),
                    elapsed_ms,
                    "warmup: done"
                );
                if was_warming {
                    self.refresh_overlay();
                }
            }
            Err(error) => {
                tracing::warn!(
                    target: "otoa_input",
                    reason = result.reason.as_str(),
                    elapsed_ms,
                    error = %error,
                    "warmup: failed"
                );
                let message = error.to_string();
                if is_user_action_failure_message(&message) {
                    self.fail_runtime_user_action(message);
                } else if was_warming {
                    self.show_overlay_error(
                        "音声認識サービスを起動できませんでした。ネットワークを確認してから、もう一度話してください。"
                            .to_string(),
                    );
                }
            }
        }
    }

    /// 保留を捨てる。**発話を失ってよいと分かっているときだけ呼ぶこと。**
    /// 停止・接続先の変更・利用者の操作待ちなど、送る先が無い場合である。
    fn clear_deferred_warmup_speech(&mut self) {
        self.deferred_speech = None;
        self.finish_after_deferred_warmup_connect = false;
    }

    /// 保留していた発話で接続を続ける。保留が無ければ何もしない。
    fn resume_deferred_warmup_speech(&mut self) {
        let Some(deferred) = self.deferred_speech.take() else {
            return;
        };
        let DeferredSpeech {
            preroll,
            audio: speech_audio,
            ended: speech_ended,
        } = deferred;

        if self.session.state() != SessionState::Listening
            || !self.session.apply(SessionInput::SpeechStarted)
        {
            return;
        }
        self.finish_after_deferred_warmup_connect = speech_ended;
        // ここから最初の応答までは、暖機と地続きの「準備中」として見せる。
        self.warmup_until_response = Some(self.warmup_started_at.unwrap_or_else(Instant::now));
        self.hold_paste_after_warmup = true;
        if let Err(error) = self.start_asr_after_speech_started(preroll, speech_audio) {
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

    /// 暖機をやり直すまでの無操作時間。
    ///
    /// **サーバーが名乗ったならそれに従う。** 認識器がいつ寝るかを知っている
    /// のは向こうで、こちらの定数は構成が変わった日に黙って合わなくなる。
    fn warmup_idle_threshold(&self) -> Duration {
        self.server_warmup_after.unwrap_or(WARMUP_IDLE_THRESHOLD)
    }

    fn warmup_is_due(&self) -> bool {
        self.provider.enrollment_is_eligible(&self.settings.core)
            && self.recently_used()
            && !matches!(self.facts.error, Some(UserError::Persistent { .. }))
            && self
                .warmup_retry_at
                .is_none_or(|retry_at| Instant::now() >= retry_at)
            && self
                .last_successful_asr_response_at
                .is_none_or(|last_response| last_response.elapsed() >= self.warmup_idle_threshold())
    }

    /// 接続先が変わったか。**変わったなら張ってある接続を切る。**
    ///
    /// 切らないと、設定も暖機も新しい方に変わるのに、音声だけが前の接続へ
    /// 流れ続ける。画面上は切り替わったように見えるので気づけない。
    fn route_differs(&self, next: &Settings) -> bool {
        self.settings.core.server_url != next.core.server_url
            || self.settings.product_settings_value() != next.product_settings_value()
    }

    /// 最近この機械で喋ったか。**暖機を続けてよいかの判断**に使う。
    ///
    /// 一度も喋っていないなら、起動しただけで待受に入っただけである。そのために
    /// 向こうの GPU を起こし続ける理由はない。
    fn recently_used(&self) -> bool {
        self.last_confident_speech_at
            .is_some_and(|at| at.elapsed() < WARMUP_ACTIVE_WINDOW)
    }

    /// 無操作中に認識器を起こす。最初の SpeechStarted まで待つと、その発話を
    /// warmup 中として捨てることになるため、Listening 中に先回りする。
    fn start_idle_warmup_if_due(&mut self) {
        if self.session.state() == SessionState::Listening
            && !self.gate.is_speaking()
            && self.warmup_is_due()
        {
            let _ = self.start_warmup(WarmupReason::Idle);
        }
    }

    /// 暖機のあと、保留した発話の最初の応答を待っているあいだ。
    ///
    /// 応答が来たか、待ちが無くなったら終わる。
    fn waiting_after_warmup(&mut self) -> Option<Instant> {
        let started_at = self.warmup_until_response?;
        if !matches!(self.server_turn, ServerTurn::AwaitingFirstResponse { .. })
            || self.facts.partial.is_some()
        {
            self.warmup_until_response = None;
            return None;
        }
        Some(started_at)
    }

    fn is_warming_overlay(&self) -> bool {
        self.facts.warmup.is_some()
            || matches!(
                self.overlay,
                OverlayView::Shown {
                    kind: OverlayKind::WarmingUp,
                    ..
                }
            )
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
        let readiness = self.connection_readiness();
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
        self.facts.error = Some(UserError::Persistent { kind, message });
        self.refresh_overlay();
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

    /// 待受を止めた。次に始めるときは、また 1 回は登録し直す。
    fn forget_warmed_since_listening(&mut self) {
        self.warmed_since_listening = false;
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
        // **待受に入ったら、まず 1 回は必ず登録する。**
        //
        // 待つ側の条件（recently_used）に任せると、一度も喋っていない起動直後は
        // 暖機しない。すると最初の発話そのものが暖機の引き金になり、毎回
        // 「まだ話さないでください」を挟むことになる。起動・方式の切り替え・
        // 声の録り直しという**決まった時点で済ませておき**、無操作が続いた場合
        // だけ、喋ったときにやり直す。
        if !self.warmed_since_listening {
            self.warmed_since_listening = true;
            if self.start_warmup(WarmupReason::Startup) {
                return;
            }
        }
        self.start_idle_warmup_if_due();
    }

    fn disable_listening(&mut self) {
        self.forget_warmed_since_listening();
        // blocking enrollment は止められない場合がある。receiver を外し epoch を
        // 進めることで、Disabled になった後の結果を必ず捨てる。
        self.cancel_warmup();
        self.suspend_vad();
        self.audio_capture.take();
        self.gate.reset();
        self.preroll.clear();
        self.level_clip_window.clear();
        self.clear_server_turn("listening disabled");

        match self.session.state() {
            SessionState::Listening => {
                if self.session.apply(SessionInput::Disable) {
                    self.send_ui(UiUpdate::State(SessionState::Disabled));
                }
            }
            SessionState::Connecting | SessionState::Streaming => {
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
        self.clear_overlay_facts();
        self.refresh_overlay();
    }

    /// 同梱サーバーを立てる。必要なら認識モデルを自動ダウンロードし、進捗を
    /// オーバーレイへ出す。**起動時に一度だけ呼ぶ。** 認識エンジンの変更は
    /// 再起動後に反映されるので、切り替え後もこの経路を通る。
    fn bootstrap_bundled_server(&mut self) {
        let Ok(endpoint) = self.provider.endpoint_hint(&self.settings.core) else {
            return;
        };
        let engine = self.settings.asr_engine.clone();
        // 進捗表示は set_overlay を通さず直接 UI へ送る（ダウンロード中は
        // &mut self を握れないため）。表示は完了後に本来の状態へ戻す。
        let to_ui = self.to_ui.clone();
        let mut last_percent: Option<u8> = None;
        let result = bundled_server::start_if_needed(&endpoint.url, &engine, &mut |status| {
            if status.total == 0 {
                return;
            }
            let percent = ((status.downloaded.min(status.total) * 100) / status.total) as u8;
            if last_percent == Some(percent) {
                return;
            }
            last_percent = Some(percent);
            let to_mib = |bytes: u64| (bytes as f64) / (1024.0 * 1024.0);
            let message = format!(
                "認識モデルを準備しています… {percent}%（{:.0} / {:.0} MB）",
                to_mib(status.downloaded),
                to_mib(status.total)
            );
            let _ = to_ui.try_send(UiUpdate::Overlay(OverlayView::Shown {
                kind: OverlayKind::Preparing,
                committed: message,
                partial: String::new(),
                error: String::new(),
            }));
        });
        match result {
            Ok(Some(model_dir)) => {
                tracing::info!(model_dir = %model_dir.display(), "同梱の ASR サーバーを起動した");
            }
            Ok(None) => {}
            // 起動できなくても続ける。設定画面でエンジンや接続先を直せるようにする。
            Err(failure) => {
                tracing::warn!(message = %failure, "同梱の ASR サーバーを起動できない");
                self.bundled_server_failure = Some(failure.into_readiness_message());
            }
        }
        // 進捗表示で書き換えた分を、コントローラ本来の表示へ戻す。
        self.render_overlay();
    }

    fn update_settings(&mut self, settings: Settings) {
        let microphone_changed = self.settings.microphone != settings.microphone;
        // 接続先が変わったなら、その場で暖める。**保存した直後だけ遅い**のを避ける。
        let route_changed = self.route_differs(&settings);
        configure_text_output(&mut self.text_out, &settings);
        let product_settings = settings.product_settings_value();
        self.provider
            .update_settings(&settings.core, product_settings.as_ref());
        self.send_route_update_for(&settings);
        if matches!(
            self.session.state(),
            SessionState::Disabled | SessionState::Failed
        ) {
            let should_enable = self.session.state() == SessionState::Disabled
                && !self.settings.listening_enabled
                && settings.listening_enabled;
            self.settings = settings;
            self.rebuild_vad_configuration();
            if route_changed {
                // **張ってある接続を切る。** 接続先が変わっても切らないと、
                // 音声は前の接続へ流れ続ける。設定も暖機も新しい方に変わるので、
                // 画面上は切り替わったように見えるのに、実際は前の経路で
                // 処理される。**実際にそうなった。**
                self.cleanup_asr();
                // **設定が実際に効いた後に打つ。** 前に打つと、暖めるのは
                // 切り替える前の接続先になる。
                let _ = self.start_warmup(WarmupReason::SettingsChanged);
            }
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
            if route_changed {
                // **接続先が変わったら、待たずに効かせる。**
                //
                // 保留すると、効くのは今のセッションが終わったときになる。待受
                // 中はセッションが終わらないので、**保存したのに何も変わらない**。
                // 起動し直したときだけ切り替わる、という形になっていた。
                //
                // 途中の発話は失われるが、利用者は文字にする方法を変えたので
                // あって、いまの発話を続けたいわけではない。
                self.settings = settings;
                self.rebuild_vad_configuration();
                self.cleanup_asr();
                let _ = self.start_warmup(WarmupReason::SettingsChanged);
                self.refresh_overlay();
                return;
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

        const CONFIDENT_SPEECH_PROB: f32 = 0.9;
        if frame
            .probs
            .iter()
            .copied()
            .any(|prob| prob >= CONFIDENT_SPEECH_PROB)
        {
            self.last_confident_speech_at = Some(Instant::now());
        }

        let threshold = self.settings.vad_threshold;
        for (index, prob) in frame.probs.iter().copied().enumerate() {
            let voiced = prob >= threshold;
            if voiced == self.vad_frame_voiced {
                continue;
            }
            if voiced {
                tracing::debug!(
                    target: "otoa_input",
                    index,
                    prob,
                    "vad edge: silent -> voiced"
                );
            } else {
                tracing::debug!(
                    target: "otoa_input",
                    index,
                    prob,
                    "vad edge: voiced -> silent"
                );
            }
            self.vad_frame_voiced = voiced;
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
                tracing::debug!(target: "otoa_input", "gate: speech started");
                self.facts.gate = GateState::Speaking;
                self.client_finalize_sent = false;
                if self.session.state() == SessionState::Streaming
                    && self.server_turn == ServerTurn::Completed
                {
                    self.server_turn = ServerTurn::Idle;
                }
                // 利用者が次を喋り始めていても、届いていない応答は前の発話の
                // ものでありうる。サーバーが実際に答えるか、セッションが終わる
                // まで、その待ちを消さない。
                self.log_server_turn_kept("next speech started");
                self.clear_commit_hold();
                self.clear_overlay_notice();
                if self.warmup_in_progress {
                    // idle warmup と同時に話し始めた場合は結果を待つ。ただし
                    // RetryableRemote ならこの音声で接続を続けるため、入力は保持する。
                    self.deferred_speech = Some(DeferredSpeech {
                        preroll: self.preroll.take(),
                        ..DeferredSpeech::default()
                    });
                    tracing::debug!(target: "otoa_input", "speech deferred while warmup is running");
                    return;
                }
                if self.session.state() == SessionState::Listening && self.warmup_is_due() {
                    self.deferred_speech = Some(DeferredSpeech {
                        preroll: self.preroll.take(),
                        ..DeferredSpeech::default()
                    });
                    if self.start_warmup(WarmupReason::Idle) {
                        return;
                    }
                    self.clear_deferred_warmup_speech();
                }
                match self.session.state() {
                    SessionState::Listening => {
                        if !self.ensure_enrolled_before_connection() {
                            return;
                        }
                        if !self.session.apply(SessionInput::SpeechStarted) {
                            return;
                        }
                        let preroll = self.preroll.take();
                        if let Err(error) = self.start_asr_after_speech_started(preroll, Vec::new())
                        {
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
                    SessionState::Streaming => {
                        // 同じ WebSocket を次の発話にも使うので、前回の `<end>`
                        // を基準に idle timeout させない。発話中の 15 秒切断も
                        // ここで防ぐ。
                        self.last_speech_endpoint_at = Some(Instant::now());
                        if self.server_audio_paused {
                            self.server_audio_paused = false;
                            self.pause_when_gate_stops = false;
                            let preroll = self.preroll.take();
                            if !preroll.is_empty() {
                                tracing::debug!(
                                    target: "otoa_input",
                                    preroll_ms = (preroll.len() * 1000)
                                        / VAD_SAMPLE_RATE as usize,
                                    "resuming server ASR audio with preroll"
                                );
                                self.send_audio(&preroll);
                            }
                        }
                        self.refresh_overlay()
                    }
                    _ => {}
                }
            }
            GateEvent::SpeechEnded => {
                tracing::debug!(
                    target: "otoa_input",
                    endpoint_mode = %self.settings.endpoint_mode,
                    "gate: speech ended"
                );
                self.facts.gate = GateState::Idle;
                if let Some(deferred) = self.deferred_speech.as_mut() {
                    // enrollment がまだ走っている。終話まで見届けたことを覚えて
                    // おき、warmup が終わったところで送り切る。
                    deferred.ended = true;
                    tracing::debug!(target: "otoa_input", "speech ended while warmup is running");
                    return;
                }
                self.finish_current_speech()
            }
        }
    }

    /// 接続前 enrollment の唯一の同期入口。リモート失敗は gateway 側の回復へ
    /// 任せて接続を続け、端末でしか直せない不足だけを永続エラーにする。
    fn ensure_enrolled_before_connection(&mut self) -> bool {
        match self
            .provider
            .ensure_enrolled(&self.settings.core, EnrollReason::BeforeConnection)
        {
            EnrollOutcome::Ready => true,
            EnrollOutcome::RetryableRemote(message) => {
                tracing::warn!(
                    target: "otoa_input",
                    error = %message,
                    "enrollment before connection failed remotely; continuing"
                );
                true
            }
            EnrollOutcome::NeedsUserAction(message) => {
                self.fail_runtime_user_action(message);
                false
            }
        }
    }

    /// `SessionInput::SpeechStarted` 後に ASR を開く共通処理。`deferred_audio` は
    /// retryable warmup failure 中に保持した音声で、Connected 後に送信される。
    fn start_asr_after_speech_started(
        &mut self,
        preroll: Vec<i16>,
        deferred_audio: Vec<Vec<i16>>,
    ) -> anyhow::Result<()> {
        let now = Instant::now();
        self.session_started_at = Some(now);
        self.connecting_started_at = Some(now);
        self.closing_started_at = None;
        self.last_speech_endpoint_at = Some(now);
        self.sent_audio_ms = 0;
        self.last_audio_log_at = Some(now);
        self.log_session_event("SpeechStarted");
        tracing::debug!(
            target: "otoa_input",
            preroll_ms = (preroll.len() * 1000) / VAD_SAMPLE_RATE as usize,
            capacity_ms = self.settings.preroll_ms,
            deferred_frames = deferred_audio.len(),
            "session preroll"
        );
        self.start_asr(preroll)?;
        self.pending_audio.extend(
            deferred_audio
                .into_iter()
                .map(|samples| samples_to_bytes(&samples)),
        );
        Ok(())
    }

    /// endpoint_mode = client では端末 VAD が終話を一度だけ
    /// 決める。遅延 warmup 後に Connected になった場合にも同じ処理を使う。
    fn finish_current_speech(&mut self) {
        if self.settings.endpoint_mode == "client"
            && self.session.state() == SessionState::Streaming
            && !self.client_finalize_sent
        {
            self.client_finalize_sent = true;
            tracing::debug!(target: "otoa_input", "sending finalize");
            if self.send_finalize() {
                self.finalize_pending = true;
            } else {
                tracing::debug!(
                    target: "otoa_input",
                    reason = "send failed",
                    "finalize skipped"
                );
            }
        } else if self.settings.endpoint_mode == "server"
            && self.session.state() == SessionState::Streaming
        {
            // ここでサーバーへ何かを送ることはしない。**端末の VAD が決めて
            // よいのは「話し始め」だけ**で、発話が終わったかどうかは ASR
            // サーバーが判断する。端末が「終わった」と伝えると、その判断を
            // 端末が肩代わりすることになる。
            //
            // ただし表示は端末の都合で先に動かしてよい。応答を待っている
            // ことを利用者へ見せるのは、判断ではなく見せ方の話である。
            //
            // ただし前の `<end>` で止め損ねた送信はここで止める。止めないと
            // VAD を通さない音がサーバーへ流れ続ける。
            if self.pause_when_gate_stops {
                self.pause_when_gate_stops = false;
                self.server_audio_paused = true;
                self.preroll.clear();
                tracing::debug!(
                    target: "otoa_input",
                    "paused server ASR audio after deferred endpoint"
                );
            }
            self.start_server_response_wait();
        } else if self.settings.endpoint_mode == "client" {
            let reason = if self.session.state() != SessionState::Streaming {
                "session is not streaming"
            } else {
                "finalize already sent"
            };
            tracing::debug!(target: "otoa_input", reason, "finalize skipped");
        } else if self.settings.endpoint_mode != "server" {
            tracing::debug!(
                target: "otoa_input",
                reason = "unsupported endpoint mode",
                "speech end ignored"
            );
        }
        self.refresh_overlay();
    }

    fn handle_vad_samples(&mut self, samples: &[i16]) {
        if let Some(deferred) = self.deferred_speech.as_mut() {
            // warmup が終わったら、これで接続を続ける。最大 180 秒の enrollment
            // timeout でも数 MB 程度で、音声を失うより小さい。
            // Disabled / cancel 時は即座に clear する。
            deferred.audio.push(samples.to_vec());
            return;
        }
        if self.warmup_in_progress {
            // idle warmup 中は SpeechStarted が上で deferred 状態へ切り替えるまで
            // 入力を ASR に渡さない。
            return;
        }
        match self.session.state() {
            SessionState::Listening | SessionState::Failed => {
                self.preroll.push(samples);
            }
            SessionState::Connecting => {
                self.queue_pending_audio(samples_to_bytes(samples));
            }
            SessionState::Streaming => {
                if self.server_audio_paused {
                    // `<end>` の後に無音を送り続けると、サーバー側が同じ入力を
                    // 新しい空発話として終話し続ける。次の SpeechStarted に備え
                    // て、容量が限られたプリロールだけを保つ。
                    self.preroll.push(samples);
                } else {
                    self.send_audio(samples);
                }
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
        self.clear_server_turn("ASR connection started");
        self.server_audio_paused = false;
        self.pause_when_gate_stops = false;
        let endpoint = self.provider.endpoint(&self.settings.core)?;

        let config_key = endpoint
            .headers
            .is_empty()
            .then(|| endpoint.api_key.clone())
            .flatten();
        let mut config =
            AsrConfig::realtime_pcm16k(config_key).with_endpoint_mode(&self.settings.endpoint_mode);
        config.language_hints = self.settings.language_hints.clone();
        let (to_asr, commands) = crossbeam_channel::unbounded();
        let (events, asr_events) = crossbeam_channel::unbounded();
        let asr_thread =
            AsrSession::spawn(endpoint.url, config, endpoint.headers, commands, events)?;

        self.active_api_key = endpoint.api_key;
        self.asr_closing = false;
        self.to_asr = Some(to_asr);
        self.asr_events = Some(asr_events);
        self.asr_thread = Some(asr_thread);
        self.pending_audio.clear();
        self.pending_audio_dropped_frames = 0;
        self.facts.error = None;
        self.facts.notice = None;
        if !preroll.is_empty() {
            self.pending_audio.push(samples_to_bytes(&preroll));
        }
        self.splash_started_at = None;
        self.refresh_overlay();
        self.send_ui(UiUpdate::State(SessionState::Connecting));
        Ok(())
    }

    fn queue_pending_audio(&mut self, bytes: Vec<u8>) {
        if self.pending_audio.len() >= PENDING_AUDIO_LIMIT {
            self.pending_audio_dropped_frames += 1;
            if self.pending_audio_dropped_frames == 1
                || self.pending_audio_dropped_frames.is_multiple_of(10)
            {
                tracing::warn!(
                    target: "otoa_input",
                    pending_limit = PENDING_AUDIO_LIMIT,
                    dropped_frames = self.pending_audio_dropped_frames,
                    "ASR connection is not ready; dropping oldest pending audio frame"
                );
            }
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

    fn start_server_response_wait(&mut self) {
        if self.session.state() != SessionState::Streaming {
            return;
        }
        match self.server_turn {
            ServerTurn::Idle => {
                self.server_turn = ServerTurn::AwaitingFirstResponse {
                    since: Instant::now(),
                };
                tracing::debug!(target: "otoa_input", "サーバーの最初の応答を待ち始めた");
            }
            ServerTurn::AwaitingFirstResponse { .. }
            | ServerTurn::Receiving { .. }
            | ServerTurn::Completed => self.log_server_turn_kept("local speech ended"),
        }
    }

    /// 発話が次へ進んでも、既に表示しているサーバー応答待ちを保持したことを記録する。
    /// gate event ごとに高頻度で出るものではないため、再表示・タイマー再始動の抑制を
    /// 実機ログから追える。
    fn log_server_turn_kept(&self, reason: &'static str) {
        let (phase, elapsed) = match self.server_turn {
            ServerTurn::AwaitingFirstResponse { since } => ("awaiting-first", since.elapsed()),
            ServerTurn::Receiving { last_activity_at } => ("receiving", last_activity_at.elapsed()),
            ServerTurn::Idle | ServerTurn::Completed => return,
        };
        tracing::info!(
            target: "otoa_input",
            elapsed_ms = elapsed.as_millis() as u64,
            phase,
            reason,
            "overlay: waiting kept"
        );
    }

    /// 応答イベントを一つの入口で server turn の相へ反映する。
    fn observe_server_activity(&mut self, activity: ServerActivity, reason: &'static str) {
        if self.session.state() != SessionState::Streaming {
            self.server_turn = ServerTurn::Idle;
            return;
        }
        let previous = self.server_turn;
        self.server_turn = match activity {
            // Completed は次の SpeechStarted まで吸収相にする。サーバー終話が端末
            // 終話より先に来たあと、同じ応答の文字イベントで待ちを再開しない。
            ServerActivity::Response if previous == ServerTurn::Completed => previous,
            ServerActivity::Response => ServerTurn::Receiving {
                last_activity_at: Instant::now(),
            },
            ServerActivity::Completed => ServerTurn::Completed,
            ServerActivity::Ended => ServerTurn::Idle,
        };
        tracing::debug!(
            target: "otoa_input",
            from = ?previous,
            to = ?self.server_turn,
            reason,
            "server activity"
        );
    }

    /// セッションの終了・切断時には、対応先を失った turn を取り除く。
    fn clear_server_turn(&mut self, reason: &'static str) {
        if self.server_turn != ServerTurn::Idle {
            tracing::debug!(target: "otoa_input", turn = ?self.server_turn, reason, "server turn cleared");
        }
        self.server_turn = ServerTurn::Idle;
    }

    /// 100 ms tick と既存テストからの明示的な再評価口。表示の選択は `view` だけが行う。
    fn check_server_turn_overlay(&mut self) {
        self.render_overlay();
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
        if !matches!(&event, AsrEvent::Failed(_) | AsrEvent::Closed { .. }) {
            // 接続確立や文字列・終話の応答が来ていれば、サービスは直近まで使われて
            // いた。次の発話を 60 秒未満で始める限り、余計な warmup はしない。
            self.last_successful_asr_response_at = Some(Instant::now());
        }
        match event {
            AsrEvent::Connected => {
                if !self.session.apply(SessionInput::Connected) {
                    return;
                }
                self.expected_backend = self.provider.expected_backend(&self.settings.core);
                self.backend_confirmed = self.expected_backend.is_none();
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
                if self.finish_after_deferred_warmup_connect {
                    self.finish_after_deferred_warmup_connect = false;
                    self.finish_current_speech();
                } else {
                    self.refresh_overlay();
                }
                self.log_session_event("Connected");
            }
            AsrEvent::FinalText(_) | AsrEvent::PartialText(_) if !self.backend_confirmed => {
                // **名乗りが来ないサーバーからは 1 文字も受け取らない。**
                // 指定を知らない古いゲートウェイは、その指定を黙って捨てて
                // 別の経路で処理する。黙って落ちるくらいなら繋がらないほうがよい。
                tracing::error!(
                    target: "otoa_input",
                    expected = ?self.expected_backend,
                    "transcript arrived before the backend was confirmed"
                );
                self.fail_runtime_user_action(
                    "選んだ文字にする方法が使われていません。接続先を確認してください。"
                        .to_string(),
                );
            }
            AsrEvent::FinalText(tokens) => {
                // finalize と同じ応答に含まれる FinalText は現在の発話の
                // 確定結果なので受け取る。FinalizeDone 後、次の発話前に届く
                // 遅れた結果は次の区切りへ混ざるため捨てる。
                if self.client_finalize_sent && !self.gate.is_speaking() && !self.finalize_pending {
                    tracing::debug!(
                        target: "otoa_input",
                        "ignored final text after client finalize until next speech"
                    );
                    return;
                }
                // FinalText はそれ自体がサーバーからの応答である。終話の印を
                // 待つ間は Receiving とし、最初の応答待ちを残さない。
                self.observe_server_activity(ServerActivity::Response, "final transcript received");
                self.transcript.push_final(&tokens_to_text(&tokens));
                self.send_text_update(true);
            }
            AsrEvent::PartialText(tokens) => {
                if self.client_finalize_sent && !self.gate.is_speaking() {
                    tracing::debug!(
                        target: "otoa_input",
                        "ignored partial text after client finalize until next speech"
                    );
                    return;
                }
                self.observe_server_activity(
                    ServerActivity::Response,
                    "partial transcript received",
                );
                let text = tokens_to_text(&tokens);
                let had_commit_hold = !text.is_empty() && self.clear_commit_hold();
                self.transcript.replace_partial(&text);
                self.send_text_update(had_commit_hold);
            }
            AsrEvent::Endpoint => {
                self.observe_server_activity(ServerActivity::Completed, "endpoint received");
                self.finalize_pending = false;
                self.last_speech_endpoint_at = Some(Instant::now());
                if self.settings.endpoint_mode == "server" && self.gate.is_speaking() {
                    // 次の発話を拾っている最中なので今は止められない。VAD が
                    // 黙ったら止める。
                    self.pause_when_gate_stops = true;
                }
                if self.settings.endpoint_mode == "server" && !self.gate.is_speaking() {
                    // `<end>` を受けた後は次の SpeechStarted まで送信を止める。
                    // ただし、既に次の発話を拾っている場合は古い `<end>` の可能性
                    // があるため止めず、その発話を欠かさない。
                    self.server_audio_paused = true;
                    self.preroll.clear();
                    tracing::debug!(target: "otoa_input", "paused server ASR audio after endpoint");
                }
                self.log_session_event("SpeechEndpoint");
                let segment = self.transcript.take_segment();
                self.commit_segment(segment);
                if self.facts.commit.is_none() && self.facts.error.is_none() {
                    self.clear_overlay_facts();
                    self.refresh_overlay();
                }
            }
            AsrEvent::FinalizeDone => {
                self.observe_server_activity(
                    ServerActivity::Completed,
                    "finalize response received",
                );
                // endpoint_mode=client では <end> が来ないので、ここで区切り時刻を更新する。
                // 更新しないと last_speech_endpoint_at が発話開始のまま止まり、
                // idle_close_sec を過ぎた後は毎周期 finalize を送り続ける。
                self.finalize_pending = false;
                self.last_speech_endpoint_at = Some(Instant::now());
                self.log_session_event("FinalizeDone");
                let segment = self.transcript.take_segment();
                self.commit_segment(segment);
                if self.facts.commit.is_none() {
                    self.refresh_overlay();
                }
            }
            AsrEvent::Finished => {
                self.observe_server_activity(ServerActivity::Ended, "session finished");
                self.finalize_pending = false;
                self.asr_closing = true;
                self.log_session_event("Finished");
                let segment = self.transcript.take_segment();
                self.commit_segment(segment);
                if self.facts.commit.is_none() {
                    self.refresh_overlay();
                }
                if self.session.apply(SessionInput::Finished) {
                    let next_state = self.session.state();
                    self.send_ui(UiUpdate::State(next_state));
                    self.cleanup_asr();
                    self.resume_after_finished();
                    self.apply_pending_settings();
                    self.refresh_overlay();
                }
            }
            AsrEvent::Backend(actual) => {
                match self.expected_backend.as_deref() {
                    Some(expected) if expected == actual => {
                        self.backend_confirmed = true;
                        tracing::info!(
                            target: "otoa_input",
                            backend = %actual,
                            "backend confirmed"
                        );
                    }
                    Some(expected) => {
                        // **選んでいない経路で処理されている。** 文字を受け取る前に切る。
                        tracing::error!(
                            target: "otoa_input",
                            expected = %expected,
                            actual = %actual,
                            "backend mismatch"
                        );
                        self.fail_runtime_user_action(
                            "選んだ文字にする方法が使われていません。接続先を確認してください。"
                                .to_string(),
                        );
                    }
                    None => {}
                }
            }
            AsrEvent::Closed { code, reason } => {
                self.observe_server_activity(ServerActivity::Ended, "ASR WebSocket closed");
                if code == Some(POLICY_VIOLATION_CLOSE_CODE) {
                    let message = if reason.is_empty() {
                        "音声認識サービスへの接続が拒否されました".to_string()
                    } else {
                        reason
                    };
                    tracing::warn!(?code, "ASR connection rejected by server");
                    self.fail_runtime(message);
                } else if self.asr_closing || self.session.state() == SessionState::Closing {
                    tracing::debug!(?code, "ASR connection closed during normal shutdown");
                    self.finish_asr_shutdown();
                } else if !reason.is_empty() {
                    self.fail_runtime(reason);
                } else {
                    self.abort_asr_session(AsrError::ClosedEarly);
                }
            }
            AsrEvent::WarmupAfter(after) => {
                // **サーバーが決めた間隔を使う。** 届かない配布では既定のまま。
                if self.server_warmup_after != Some(after) {
                    tracing::info!(
                        target: "otoa_input",
                        secs = after.as_secs(),
                        "warmup interval announced by the server"
                    );
                }
                self.server_warmup_after = Some(after);
            }
            AsrEvent::Notice { code, message } => {
                self.observe_server_activity(ServerActivity::Completed, "notice received");
                self.show_overlay_notice(code, message);
            }
            AsrEvent::Failed(error) => {
                self.observe_server_activity(ServerActivity::Ended, "ASR error received");
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
        self.clear_overlay_facts();
        self.send_ui(UiUpdate::State(self.session.state()));
        self.resume_after_finished();
        self.apply_pending_settings();
        self.refresh_overlay();
    }

    fn abort_asr_session(&mut self, error: AsrError) {
        tracing::warn!("ASR session ended normally: {error}");
        self.send_stop();
        self.cleanup_asr();
        let _ = self.transcript.take_segment();
        if !self.session.apply(SessionInput::Aborted) {
            return;
        }
        self.clear_overlay_facts();
        self.send_ui(UiUpdate::State(SessionState::Listening));
        self.resume_after_finished();
        self.apply_pending_settings();
        self.refresh_overlay();
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
        self.check_server_turn_overlay();
        self.start_idle_warmup_if_due();
        if !idle_close_is_due(
            self.session.state(),
            self.gate.is_speaking(),
            self.server_turn,
            self.last_speech_endpoint_at,
            self.settings.idle_close_sec,
            Instant::now(),
        ) {
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
            self.clear_overlay_facts();
            self.send_ui(UiUpdate::State(SessionState::Closing));
            self.refresh_overlay();
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
        self.clear_overlay_facts();
        self.send_ui(UiUpdate::State(SessionState::Listening));
        tracing::info!("session recovered to Listening");
        self.resume_after_finished();
        self.apply_pending_settings();
        self.refresh_overlay();
    }

    fn reset_after_session_timeout(&mut self) {
        self.send_stop();
        self.cleanup_asr();
        if !self.session.apply(SessionInput::Timeout) {
            return;
        }
        self.clear_overlay_facts();
        self.send_ui(UiUpdate::State(SessionState::Listening));
        self.reset_vad_state();
        if self.audio_capture.is_none() {
            if let Err(error) = self.start_audio_capture() {
                self.suspend_vad();
                self.fail_runtime_user_action(format!("failed to resume microphone: {error:#}"));
            }
        }
        self.apply_pending_settings();
        self.refresh_overlay();
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
        // 暖機に待たされた発話は貼らない。待っているあいだに利用者は別の窓へ
        // 移っていることがあり、そこへ貼ると取り消せない。
        let method = if std::mem::take(&mut self.hold_paste_after_warmup) {
            tracing::info!(
                target: "otoa_input",
                "暖機で待たせた発話なので、貼らずにクリップボードへ置いた"
            );
            PasteMethod::ClipboardOnly
        } else {
            PasteMethod::ClipboardAndPaste
        };
        if let Err(error) = self.text_out.emit(&text, method) {
            self.report_error(format!("failed to output transcript: {error:#}"));
        }
        // 利用者が体感する遅延。起点は「最後に確実に声だったフレーム」であって、
        // `vad edge`(閾値 0.5 を割った瞬間)ではない。ここを取り違えると、
        // 報告値と体感が食い違う。
        if let Some(spoke_at) = self.last_confident_speech_at.take() {
            tracing::info!(
                target: "otoa_input",
                total_ms = spoke_at.elapsed().as_millis() as u64,
                chars = text.chars().count(),
                "user latency: last confident speech -> pasted"
            );
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
        self.import_display_deadline_overrides_for_test();
        let now = Instant::now();
        let mut changed = false;
        if matches!(
            self.facts.error,
            Some(UserError::Temporary { until, .. }) if now >= until
        ) {
            self.facts.error = None;
            changed = true;
        }
        if self
            .facts
            .notice
            .as_ref()
            .is_some_and(|notice| now >= notice.until)
        {
            self.facts.notice = None;
            changed = true;
        }
        if changed {
            self.render_overlay();
        }
    }

    fn check_commit_hold_timeout(&mut self) {
        self.import_display_deadline_overrides_for_test();
        if self
            .facts
            .commit
            .as_ref()
            .is_some_and(|(_, deadline)| Instant::now() >= *deadline)
        {
            self.facts.commit = None;
            self.render_overlay();
        }
    }

    fn check_splash_timeout(&mut self) {
        if self.splash_started_at.is_none() {
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
            self.render_overlay();
        }
    }

    fn show_overlay_error(&mut self, message: String) {
        let until = Instant::now() + OVERLAY_ERROR_DURATION;
        self.splash_started_at = None;
        self.facts.notice = None;
        self.facts.error = Some(UserError::Temporary { message, until });
        self.refresh_overlay();
    }

    fn show_overlay_notice(&mut self, code: String, message: String) {
        let message = self.sanitize_message(message);
        let until = Instant::now() + OVERLAY_NOTICE_DURATION;
        self.splash_started_at = None;
        self.facts.error = None;
        self.facts.commit = None;
        self.facts.notice = Some(Notice {
            kind: NoticeKind::Asr,
            message,
            until,
        });
        tracing::info!(target: "otoa_input", notice_code = %code, "ASR notice");
        self.refresh_overlay();
    }

    fn show_persistent_overlay_error(&mut self, message: String) {
        self.clear_server_turn("persistent error shown");
        self.splash_started_at = None;
        self.facts.notice = None;
        self.facts.error = Some(UserError::Persistent {
            kind: OverlayKind::Error,
            message: format!("{message}\nクリックで再試行"),
        });
        self.refresh_overlay();
    }

    /// セッション切替時に、期限付きの可視理由を Facts から取り除く。
    fn clear_overlay_facts(&mut self) {
        self.splash_started_at = None;
        self.facts.error = None;
        self.facts.notice = None;
        self.facts.commit = None;
    }

    fn clear_commit_hold(&mut self) -> bool {
        let was_present = self.facts.commit.is_some();
        self.facts.commit = None;
        was_present
    }

    fn clear_overlay_notice(&mut self) -> bool {
        let was_present = self.facts.notice.is_some();
        self.facts.notice = None;
        was_present
    }

    fn show_committed_text(&mut self, text: String) {
        if self.settings.commit_hold_ms == 0 {
            self.facts.commit = None;
            self.refresh_overlay();
            return;
        }
        let until = Instant::now() + Duration::from_millis(u64::from(self.settings.commit_hold_ms));
        self.splash_started_at = None;
        self.facts.error = None;
        self.facts.notice = None;
        self.facts.commit = Some((text, until));
        self.refresh_overlay();
    }

    fn login_required(&self) -> bool {
        matches!(self.provider.readiness(), Readiness::NeedsLogin { .. })
    }

    fn connection_needs_attention(&self) -> bool {
        !matches!(self.connection_readiness(), Readiness::Ready)
    }

    fn connection_readiness(&self) -> Readiness {
        combine_readiness(
            self.provider.readiness(),
            self.bundled_server_failure.as_deref(),
        )
    }

    fn refresh_overlay(&mut self) {
        // splash は通常イベント後に表示対象ではなくなる。接続 readiness の
        // 確認タイマーも従来どおり停止する。
        self.splash_started_at = None;
        self.render_overlay();
    }

    /// 表示に依存する実行時の事実だけを更新する。期限・待機列はここで復元せず、
    /// それぞれのイベントが Facts を直接更新する。
    fn refresh_runtime_facts(&mut self) {
        self.facts.session = self.session.state();
        self.facts.gate = if self.gate.is_speaking() {
            GateState::Speaking
        } else {
            GateState::Idle
        };
        // 暖機中と、そこから続く「最初の応答待ち」を、ひと続きで見せる。
        self.facts.warmup = match self.warmup_started_at {
            Some(started_at) if self.warmup_in_progress => Some(started_at),
            _ => self.waiting_after_warmup(),
        };

        self.facts.partial =
            (!self.transcript.partial().is_empty()).then(|| self.transcript.partial().to_string());
        self.facts.finalizing = self.finalize_pending;
    }

    fn render_overlay(&mut self) {
        self.refresh_runtime_facts();
        self.set_overlay(view(
            &self.facts,
            self.server_turn,
            self.connecting_started_at,
            Instant::now(),
        ));
        self.sync_display_deadline_observer();
    }

    #[cfg(test)]
    fn import_display_deadline_overrides_for_test(&mut self) {
        if let (Some(until), Some((_, fact_until))) =
            (self.committed_hold_until, self.facts.commit.as_mut())
        {
            *fact_until = until;
        }
        if let (Some(until), Some(notice)) = (self.overlay_notice_until, self.facts.notice.as_mut())
        {
            notice.until = until;
        }
        if let (
            Some(until),
            Some(UserError::Temporary {
                until: fact_until, ..
            }),
        ) = (self.overlay_error_until, self.facts.error.as_mut())
        {
            *fact_until = until;
        }
    }

    #[cfg(not(test))]
    fn import_display_deadline_overrides_for_test(&mut self) {}

    #[cfg(test)]
    fn sync_display_deadline_observer(&mut self) {
        self.committed_hold_until = self.facts.commit.as_ref().map(|(_, until)| *until);
        self.overlay_notice_until = self.facts.notice.as_ref().map(|notice| notice.until);
        self.overlay_error_until = match self.facts.error.as_ref() {
            Some(UserError::Temporary { until, .. }) => Some(*until),
            Some(UserError::Persistent { .. }) | None => None,
        };
    }

    #[cfg(not(test))]
    fn sync_display_deadline_observer(&mut self) {}

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

    fn send_route_update(&self) {
        self.send_route_update_for(&self.settings);
    }

    fn send_route_update_for(&self, settings: &Settings) {
        let Ok(endpoint) = self.provider.endpoint_hint(&settings.core) else {
            return;
        };
        let Ok(url) = Url::parse(&endpoint.url) else {
            return;
        };
        let Some(host) = url.host_str() else {
            return;
        };
        self.send_ui(UiUpdate::Route {
            local: is_loopback_host(host),
        });
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
        configure_text_output(&mut self.text_out, &settings);
        self.settings = settings;
        self.rebuild_vad_configuration();
        if std::mem::take(&mut self.warmup_after_pending_settings) {
            // 保留していた設定が効いたときも、接続を張り直す。理由は同じ。
            self.cleanup_asr();
            let _ = self.start_warmup(WarmupReason::SettingsChanged);
        }
        if was_enabled && !self.settings.listening_enabled {
            self.disable_listening();
        }
    }

    fn request_shutdown(&mut self) {
        self.cancel_warmup();
        let was_closing = self.session.state() == SessionState::Closing;
        self.disable_listening();
        if was_closing {
            self.send_stop();
        }
        let _ = self.vad_control.send(VadControl::Shutdown);
    }

    fn cleanup_asr(&mut self) {
        self.clear_server_turn("ASR session cleaned up");
        self.finalize_pending = false;
        self.client_finalize_sent = false;
        self.server_audio_paused = false;
        self.pause_when_gate_stops = false;
        self.to_asr = None;
        self.asr_events = None;
        if let Some(session_thread) = self.asr_thread.take() {
            let _ = session_thread.join();
        }
        self.pending_audio.clear();
        self.pending_audio_dropped_frames = 0;
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

fn next_enroll_retry_delay(current: Duration) -> Duration {
    current
        .checked_mul(2)
        .unwrap_or(ENROLL_RETRY_MAX)
        .min(ENROLL_RETRY_MAX)
}

fn failed_retry_is_due(failed_at: Instant, retry_delay: Duration, now: Instant) -> bool {
    now.duration_since(failed_at) >= retry_delay
}

/// 無音が続いたので接続を閉じてよいか。
///
/// **応答をまだ待っている間は、turn の期限までは閉じない。** 2026-08-25 の実機で、喋り終わってから
/// 15 秒(`idle_close_sec` の既定)でセッションを閉じてしまい、背後の
/// コールドスタート(30〜60 秒)が終わる前に諦めていた。閉じることすら
/// 完了せず `closing timed out without finished` になり、未応答の待ちが
/// 破棄されていた。その猶予は残しつつ、無応答なら期限後に閉じられるようにする。
fn idle_close_is_due(
    state: SessionState,
    gate_is_speaking: bool,
    server_turn: ServerTurn,
    last_speech_endpoint_at: Option<Instant>,
    idle_close_sec: u32,
    now: Instant,
) -> bool {
    state == SessionState::Streaming
        && !gate_is_speaking
        && !server_turn.blocks_idle_close(now)
        && last_speech_endpoint_at.is_some_and(|last_endpoint| {
            now.duration_since(last_endpoint) > Duration::from_secs(u64::from(idle_close_sec))
        })
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
    ) || message.starts_with("声の登録が必要です。")
        || message.starts_with("参照音声が見つかりません。")
        || message.starts_with("声の登録の準備に失敗しました。")
}

/// server endpoint でも無音の待機時間を短縮しない。100 ms の上限では、VAD 確率が
/// 境界付近で振動しただけで再武装して連続した SpeechEnded を起こすため、
/// endpoint の担当にかかわらず設定した `vad_min_silence_ms` をそのまま使う。
fn gate_from_settings(settings: &Settings) -> SpeechGate {
    SpeechGate::new(
        settings.vad_threshold,
        settings.vad_release_threshold,
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

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::{
        apply_input_gain, combine_readiness, failed_retry_is_due, gate_from_settings,
        idle_close_is_due, is_loopback_host, is_nonfatal_asr_error, is_user_action_failure_message,
        level_status, next_enroll_retry_delay, next_failed_retry_delay, resolve_paste_shortcut,
        Controller, LevelStatus, OverlayKind, OverlayView, ServerActivity, ServerTurn,
        WarmupReason, WarmupResult, CONNECTING_STARTING_OVERLAY_DELAY, ENROLL_RETRY_INITIAL,
        ENROLL_RETRY_MAX, FAILED_RETRY_INITIAL, FAILED_RETRY_MAX, GATEWAY_URL_MISSING_MESSAGE,
        OVERLAY_NOTICE_DURATION, PENDING_AUDIO_LIMIT, SERVER_FINAL_RESPONSE_TIMEOUT,
        SERVER_FIRST_RESPONSE_TIMEOUT, SERVER_RESPONSE_WAITING_OVERLAY_DELAY,
        WARMUP_IDLE_THRESHOLD,
    };
    use crate::connection::SelfHostedProvider;
    use crate::settings::Settings;
    use otoa_input_core::{
        Account, ConnectionProvider, Endpoint, EnrollOutcome, EnrollReason, GateEvent,
        PasteShortcutSetting, PrepareAction, Readiness, SessionInput, SessionState,
    };
    use otoa_input_protocol::{
        AsrCommand, AsrError, AsrEvent, AsrToken, POLICY_VIOLATION_CLOSE_CODE,
    };
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    use std::time::{Duration, Instant};
    use url::Url;

    fn test_controller_with_failure(
        settings: Settings,
        bundled_server_failure: Option<String>,
    ) -> Controller {
        let (to_ui, _ui_rx) = crossbeam_channel::bounded(8);
        let (audio_sink, _audio_rx) = crossbeam_channel::bounded(8);
        let (vad_control, _vad_control_rx) = crossbeam_channel::bounded(8);
        let (_vad_event_tx, vad_events) = crossbeam_channel::bounded(8);
        let provider = std::sync::Arc::new(SelfHostedProvider);
        Controller::new(
            settings,
            provider,
            bundled_server_failure,
            to_ui,
            audio_sink,
            vad_control,
            vad_events,
        )
        .expect("controller should initialize for the overlay test")
    }

    fn test_controller(settings: Settings) -> Controller {
        test_controller_with_failure(settings, None)
    }

    struct WarmupProvider {
        calls: Arc<AtomicUsize>,
    }

    impl ConnectionProvider for WarmupProvider {
        fn endpoint(&self, _settings: &otoa_input_core::Settings) -> anyhow::Result<Endpoint> {
            Ok(Endpoint {
                url: "ws://127.0.0.1:8770/asr/v1".to_string(),
                headers: Vec::new(),
                api_key: None,
            })
        }

        fn supports_warmup(&self, _settings: &otoa_input_core::Settings) -> bool {
            true
        }

        fn warmup(&self, _settings: &otoa_input_core::Settings) -> anyhow::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn readiness(&self) -> Readiness {
            Readiness::Ready
        }

        fn prepare(&self) -> Option<PrepareAction> {
            None
        }

        fn authenticate(&self, _cancelled: &AtomicBool) -> anyhow::Result<()> {
            Ok(())
        }

        fn logout(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn account(&self) -> Option<Account> {
            None
        }

        fn update_settings(
            &self,
            _settings: &otoa_input_core::Settings,
            _product_settings: Option<&serde_json::Value>,
        ) {
        }
    }

    struct RetryableEnrollmentProvider {
        calls: Arc<AtomicUsize>,
    }

    impl ConnectionProvider for RetryableEnrollmentProvider {
        fn endpoint(&self, _settings: &otoa_input_core::Settings) -> anyhow::Result<Endpoint> {
            Ok(Endpoint {
                url: "ws://127.0.0.1:8770/asr/v1".to_string(),
                headers: Vec::new(),
                api_key: None,
            })
        }

        fn enrollment_is_eligible(&self, _settings: &otoa_input_core::Settings) -> bool {
            true
        }

        fn ensure_enrolled(
            &self,
            _settings: &otoa_input_core::Settings,
            _reason: EnrollReason,
        ) -> EnrollOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            EnrollOutcome::RetryableRemote("gateway timed out while starting".to_string())
        }

        fn readiness(&self) -> Readiness {
            Readiness::Ready
        }

        fn prepare(&self) -> Option<PrepareAction> {
            None
        }

        fn authenticate(&self, _cancelled: &AtomicBool) -> anyhow::Result<()> {
            Ok(())
        }

        fn logout(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn account(&self) -> Option<Account> {
            None
        }

        fn update_settings(
            &self,
            _settings: &otoa_input_core::Settings,
            _product_settings: Option<&serde_json::Value>,
        ) {
        }
    }

    struct MissingEnrollmentProvider;

    impl ConnectionProvider for MissingEnrollmentProvider {
        fn endpoint(&self, _settings: &otoa_input_core::Settings) -> anyhow::Result<Endpoint> {
            Ok(Endpoint {
                url: "ws://127.0.0.1:8770/asr/v1".to_string(),
                headers: Vec::new(),
                api_key: None,
            })
        }

        fn ensure_enrolled(
            &self,
            _settings: &otoa_input_core::Settings,
            _reason: EnrollReason,
        ) -> EnrollOutcome {
            EnrollOutcome::NeedsUserAction(
                "声の登録が必要です。設定の「声」で参照音声を録音してから、もう一度話してください。"
                    .to_string(),
            )
        }

        fn readiness(&self) -> Readiness {
            Readiness::Ready
        }

        fn prepare(&self) -> Option<PrepareAction> {
            None
        }

        fn authenticate(&self, _cancelled: &AtomicBool) -> anyhow::Result<()> {
            Ok(())
        }

        fn logout(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn account(&self) -> Option<Account> {
            None
        }

        fn update_settings(
            &self,
            _settings: &otoa_input_core::Settings,
            _product_settings: Option<&serde_json::Value>,
        ) {
        }
    }

    fn controller_with_provider(
        settings: Settings,
        provider: Arc<dyn ConnectionProvider>,
    ) -> Controller {
        let (to_ui, _ui_rx) = crossbeam_channel::bounded(8);
        let (audio_sink, _audio_rx) = crossbeam_channel::bounded(8);
        let (vad_control, _vad_control_rx) = crossbeam_channel::bounded(8);
        let (_vad_event_tx, vad_events) = crossbeam_channel::bounded(8);
        Controller::new(
            settings,
            provider,
            None,
            to_ui,
            audio_sink,
            vad_control,
            vad_events,
        )
        .expect("controller should initialize for the enrollment test")
    }

    /// 接続先が変わったことを見分けられること。
    ///
    /// **見分けられないと、前の接続へ音声が流れ続ける。** 設定も暖機も新しい
    /// 方に変わるので、画面上は切り替わったように見えて気づけない。実際に、
    /// 方法を切り替えたのに前の方法で処理され続けた。
    #[test]
    fn a_changed_product_setting_is_a_changed_route() {
        let (controller, _calls) = warmup_controller(Settings::default());
        let mut next = controller.settings.clone();
        assert!(
            !controller.route_differs(&next),
            "同じ設定なら変わっていない"
        );

        next.product = serde_json::json!({"asr_backend": "my_voice"});
        assert!(
            controller.route_differs(&next),
            "文字にする方法が変われば接続先も変わる"
        );
    }

    /// **待受中に方法を変えたら、その場で張ってある接続を切ること。**
    ///
    /// 保留すると、効くのは今のセッションが終わったときになる。待受中は
    /// セッションが終わらないので、保存したのに何も変わらない。起動し直した
    /// ときだけ切り替わる、という形になっていた。**実際にそうなった。**
    #[test]
    fn changing_the_method_while_listening_takes_effect_now() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);
        assert!(
            !matches!(
                controller.session.state(),
                SessionState::Disabled | SessionState::Failed
            ),
            "待受中であること"
        );

        let mut next = controller.settings.clone();
        next.product = serde_json::json!({"asr_backend": "my_voice_fast"});
        controller.update_settings(next);

        assert!(
            controller.pending_settings.is_none(),
            "接続先の変更は保留しない"
        );
        assert_eq!(
            controller.settings.product_settings_value(),
            Some(serde_json::json!({"asr_backend": "my_voice_fast"})),
            "新しい方法が効いていること"
        );
        assert!(controller.to_asr.is_none(), "張ってある接続を切ること");
        drop(asr_commands);
    }

    #[test]
    fn a_changed_server_url_is_a_changed_route() {
        let (controller, _calls) = warmup_controller(Settings::default());
        let mut next = controller.settings.clone();
        next.core.server_url = "wss://example.invalid/ws/asr".to_string();
        assert!(controller.route_differs(&next));
    }

    fn warmup_controller(settings: Settings) -> (Controller, Arc<AtomicUsize>) {
        let (to_ui, _ui_rx) = crossbeam_channel::bounded(8);
        let (audio_sink, _audio_rx) = crossbeam_channel::bounded(8);
        let (vad_control, _vad_control_rx) = crossbeam_channel::bounded(8);
        let (_vad_event_tx, vad_events) = crossbeam_channel::bounded(8);
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(WarmupProvider {
            calls: Arc::clone(&calls),
        });
        let controller = Controller::new(
            settings,
            provider,
            None,
            to_ui,
            audio_sink,
            vad_control,
            vad_events,
        )
        .expect("controller should initialize for the warmup test");
        (controller, calls)
    }

    fn wait_for_warmup(controller: &mut Controller) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while controller.warmup_in_progress && Instant::now() < deadline {
            controller.drain_warmup_events();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            !controller.warmup_in_progress,
            "warmup worker did not report completion"
        );
    }

    #[test]
    fn startup_warmup_shows_the_warming_overlay_until_it_finishes() {
        let (mut controller, calls) = warmup_controller(Settings::default());

        assert!(controller.start_warmup(WarmupReason::Startup));
        assert!(controller.warmup_in_progress);
        assert_eq!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WarmingUp,
                committed: String::new(),
                partial: String::new(),
                error: String::new(),
            }
        );

        wait_for_warmup(&mut controller);

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(controller.last_successful_asr_response_at.is_some());

        // 待たせている発話が無ければ、終わったら消える。
        controller.refresh_runtime_facts();
        controller.render_overlay();
        assert_eq!(controller.overlay, OverlayView::Hidden);
    }

    #[test]
    fn speech_after_sixty_seconds_warms_before_opening_a_session() {
        let (mut controller, calls) = warmup_controller(Settings::default());
        // 暖機は「最近使った」ときだけ続く。使っていない機械で
        // GPU を起こし続けないため。
        controller.last_confident_speech_at = Some(Instant::now());
        assert!(controller.session.apply(SessionInput::Enable));
        controller.last_successful_asr_response_at = Some(Instant::now() - WARMUP_IDLE_THRESHOLD);

        controller.handle_gate_event(GateEvent::SpeechStarted);

        assert!(controller.warmup_in_progress);
        assert!(controller.deferred_speech.is_some());
        assert_eq!(controller.session.state(), SessionState::Listening);
        assert!(controller.to_asr.is_none());
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WarmingUp,
                ..
            }
        ));

        wait_for_warmup(&mut controller);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// **暖機が成功しても、待たせた発話を捨てない。**
    ///
    /// 以前は成功したときだけ捨て、失敗したときだけ送っていた。利用者からは
    /// 「たまに喋っても何も起きない」に見え、暖機は 1 秒未満で終わるので
    /// 待たされたことにも気づけなかった。
    #[test]
    fn a_successful_warmup_sends_the_speech_it_held() {
        let (mut controller, _calls) = warmup_controller(Settings::default());
        controller.last_confident_speech_at = Some(Instant::now());
        assert!(controller.session.apply(SessionInput::Enable));
        controller.last_successful_asr_response_at = Some(Instant::now() - WARMUP_IDLE_THRESHOLD);
        // 喋り出しの前に溜まっている分。
        controller.preroll.push(&[2_i16; 160]);

        controller.handle_gate_event(GateEvent::SpeechStarted);
        assert!(controller.warmup_in_progress, "暖機が始まっていない");
        assert!(controller.deferred_speech.is_some(), "発話を保留していない");

        // **喋り出しの前の蓄えを預かること。** 落とすと、送られるのは語尾だけの
        // 短い音声になり、話者照合のチャンクが足りずに弾かれる。
        assert!(
            !controller
                .deferred_speech
                .as_ref()
                .expect("保留があること")
                .preroll
                .is_empty(),
            "喋り出し前の音声を捨てている"
        );

        // 暖機中に届いた音声は溜める。
        controller.handle_vad_samples(&[1_i16; 160]);
        assert_eq!(
            controller
                .deferred_speech
                .as_ref()
                .map(|deferred| deferred.audio.len()),
            Some(1),
            "溜めた音声が残っていない"
        );

        wait_for_warmup(&mut controller);

        assert!(
            controller.deferred_speech.is_none(),
            "保留が残ったままになっている"
        );
        assert_ne!(
            controller.session.state(),
            SessionState::Listening,
            "暖機のあとに発話を送っていない（捨てられている）"
        );
    }

    /// **暖機に待たされた発話は貼らない。**
    ///
    /// 暖機と接続で数秒かかることがあり、その間に利用者は別の窓へ移っている。
    /// そこへ貼ると、貼りたくない場所へ文字が入る。取り消せない。
    #[test]
    fn a_result_held_by_the_warmup_is_not_pasted() {
        let (mut controller, _calls) = warmup_controller(Settings::default());
        controller.last_confident_speech_at = Some(Instant::now());
        assert!(controller.session.apply(SessionInput::Enable));
        controller.last_successful_asr_response_at = Some(Instant::now() - WARMUP_IDLE_THRESHOLD);
        controller.preroll.push(&[2_i16; 160]);

        controller.handle_gate_event(GateEvent::SpeechStarted);
        assert!(controller.deferred_speech.is_some());
        wait_for_warmup(&mut controller);

        assert!(
            controller.hold_paste_after_warmup,
            "待たせた発話に貼り付け保留の印が付いていない"
        );
    }

    /// **待受に入ったら、まず 1 回は必ず登録する。**
    ///
    /// 待つ側の条件に任せると、一度も喋っていない起動直後は暖機しない。
    /// すると最初の発話そのものが暖機の引き金になり、毎回「まだ話さないで
    /// ください」を挟むことになる。
    #[test]
    fn listening_always_enrolls_once_before_the_first_speech() {
        let (mut controller, calls) = warmup_controller(Settings::default());
        // 一度も喋っていない＝ recently_used() は false。それでも暖機すること。
        assert!(controller.last_confident_speech_at.is_none());

        controller.enable_listening();

        assert!(controller.warmup_in_progress, "起動直後に登録していない");
        assert!(matches!(
            controller.warmup_reason,
            Some(WarmupReason::Startup)
        ));
        wait_for_warmup(&mut controller);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // 2 回目の待受開始では繰り返さない（無操作の暖機に任せる）。
        controller.enable_listening();
        assert!(
            !controller.warmup_in_progress,
            "同じ待受で二重に暖機している"
        );

        // 待受を止めたら、次に始めるときはまた 1 回通す。
        controller.disable_listening();
        controller.enable_listening();
        assert!(
            controller.warmup_in_progress,
            "止めた後に登録し直していない"
        );
    }

    /// **声を録り直したら、その場で登録し直す。**
    ///
    /// 録り直しは設定を変えないので、接続先が変わったことにはならない。
    /// 次の無操作まで待つと、そのあいだサーバーは古い声で照合し続ける。
    ///
    /// 登録の道は暖機 1 本にまとめてある。呼ぶ側が自分で資格情報を取り直して
    /// 送ると、同じことをする道が 2 つになり、片方だけ画面に出ない。
    #[test]
    fn a_re_recorded_voice_is_enrolled_through_the_same_warmup() {
        let (mut controller, calls) = warmup_controller(Settings::default());

        assert!(controller.start_warmup(WarmupReason::VoiceChanged));

        assert!(controller.warmup_in_progress);
        assert!(matches!(
            controller.warmup_reason,
            Some(WarmupReason::VoiceChanged)
        ));
        // **画面に出ること。** 無表示で登録すると、待たされている理由が
        // 利用者に分からない。
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WarmingUp,
                ..
            }
        ));

        wait_for_warmup(&mut controller);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "登録を送っていない");
    }

    /// 暖機の間隔は、サーバーが名乗ったならそれに従う。
    /// 認識器がいつ寝るかを知っているのは向こうで、こちらの定数は
    /// 向こうの構成が変わった日に黙って合わなくなる。
    #[test]
    fn the_server_decides_how_long_to_wait_before_warming_up_again() {
        let (mut controller, _calls) = warmup_controller(Settings::default());
        assert_eq!(
            controller.warmup_idle_threshold(),
            WARMUP_IDLE_THRESHOLD,
            "名乗りが無いときは既定を使う"
        );

        controller.handle_asr_event(AsrEvent::WarmupAfter(Duration::from_secs(600)));
        assert_eq!(
            controller.warmup_idle_threshold(),
            Duration::from_secs(600),
            "サーバーが名乗った間隔を使っていない"
        );

        // 既定（60 秒）なら暖機する頃合いでも、600 秒と言われたならまだ早い。
        controller.last_confident_speech_at = Some(Instant::now());
        controller.last_successful_asr_response_at = Some(Instant::now() - WARMUP_IDLE_THRESHOLD);
        assert!(
            !controller.warmup_is_due(),
            "サーバーの間隔を無視して暖機している"
        );
    }

    #[test]
    fn idle_warmup_starts_before_the_next_speech() {
        let (mut controller, calls) = warmup_controller(Settings::default());
        // 暖機は「最近使った」ときだけ続く。使っていない機械で
        // GPU を起こし続けないため。
        controller.last_confident_speech_at = Some(Instant::now());
        assert!(controller.session.apply(SessionInput::Enable));
        controller.last_successful_asr_response_at = Some(Instant::now() - WARMUP_IDLE_THRESHOLD);

        controller.start_idle_warmup_if_due();

        assert!(controller.warmup_in_progress);
        assert!(controller.deferred_speech.is_none());
        assert_eq!(controller.session.state(), SessionState::Listening);
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WarmingUp,
                ..
            }
        ));
        wait_for_warmup(&mut controller);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn temporary_warmup_failure_returns_to_a_retryable_state() {
        let (mut controller, _calls) = warmup_controller(Settings::default());
        // 暖機は「最近使った」ときだけ続く。
        controller.last_confident_speech_at = Some(Instant::now());
        controller.warmup_in_progress = true;
        controller.set_overlay(OverlayView::Shown {
            kind: OverlayKind::WarmingUp,
            committed: String::new(),
            partial: String::new(),
            error: String::new(),
        });

        controller.finish_warmup(WarmupResult {
            reason: WarmupReason::Startup,
            started_at: Instant::now(),
            result: Err(anyhow::anyhow!("gateway timed out while starting")),
        });

        assert!(!controller.warmup_in_progress);
        assert!(controller.warmup_is_due());
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Error,
                ..
            }
        ));
    }

    #[test]
    fn retryable_warmup_failure_does_not_retry_again_after_one_hundred_ms() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(RetryableEnrollmentProvider {
            calls: Arc::clone(&calls),
        });
        let mut controller = controller_with_provider(Settings::default(), provider);
        assert!(controller.session.apply(SessionInput::Enable));
        // 暖機は「最近使った」ときだけ続く。
        controller.last_confident_speech_at = Some(Instant::now());
        let before = Instant::now();

        controller.start_idle_warmup_if_due();
        wait_for_warmup(&mut controller);

        let retry_at = controller
            .warmup_retry_at
            .expect("retryable failure must schedule a retry");
        assert!(
            retry_at.duration_since(before) >= ENROLL_RETRY_INITIAL,
            "retry interval must be at least five seconds"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(controller.facts.error.is_none());
        assert_eq!(controller.session.state(), SessionState::Listening);

        std::thread::sleep(Duration::from_millis(110));
        controller.start_idle_warmup_if_due();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the 100 ms controller tick must not start a second enrollment"
        );
    }

    #[test]
    fn enrollment_retry_backoff_is_capped_at_sixty_seconds() {
        assert_eq!(
            next_enroll_retry_delay(ENROLL_RETRY_INITIAL),
            Duration::from_secs(10)
        );
        assert_eq!(
            next_enroll_retry_delay(Duration::from_secs(40)),
            ENROLL_RETRY_MAX
        );
        assert_eq!(next_enroll_retry_delay(ENROLL_RETRY_MAX), ENROLL_RETRY_MAX);
    }

    #[test]
    fn disabled_session_does_not_start_idle_warmup() {
        let (mut controller, calls) = warmup_controller(Settings::default());
        // 暖機は「最近使った」ときだけ続く。使っていない機械で
        // GPU を起こし続けないため。
        controller.last_confident_speech_at = Some(Instant::now());
        assert_eq!(controller.session.state(), SessionState::Disabled);

        controller.start_idle_warmup_if_due();

        assert!(!controller.warmup_in_progress);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn missing_enrollment_is_the_only_persistent_enrollment_error() {
        let provider = Arc::new(MissingEnrollmentProvider);
        let mut controller = controller_with_provider(Settings::default(), provider);
        assert!(controller.session.apply(SessionInput::Enable));

        controller.handle_gate_event(GateEvent::SpeechStarted);

        assert_eq!(controller.session.state(), SessionState::Failed);
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Error,
                ..
            }
        ));
        controller.check_overlay_timeout();
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Error,
                ..
            }
        ));
    }

    #[test]
    fn missing_voice_setup_is_a_user_action_failure() {
        assert!(is_user_action_failure_message(
            "声の登録が必要です。設定の「声」で参照音声を録音してから、もう一度話してください。"
        ));
        assert!(is_user_action_failure_message(
            "参照音声が見つかりません。設定の「声」で声を登録し直してから、もう一度話してください。"
        ));
    }

    #[test]
    fn auto_paste_shortcut_resolves_to_shift_insert() {
        assert_eq!(
            resolve_paste_shortcut(PasteShortcutSetting::Auto),
            otoa_input_platform::PasteShortcut::ShiftInsert
        );
    }

    #[test]
    fn bundled_server_failure_becomes_setup_readiness_when_provider_is_ready() {
        assert_eq!(
            combine_readiness(Readiness::Ready, Some("モデルが見つかりません")),
            Readiness::NeedsSetup {
                message: "モデルが見つかりません".to_string()
            }
        );
    }

    #[test]
    fn provider_readiness_takes_priority_over_bundled_server_failure() {
        let provider_readiness = Readiness::NeedsLogin {
            message: "ログインしてください".to_string(),
        };
        assert_eq!(
            combine_readiness(provider_readiness.clone(), Some("ローカルモデルエラー")),
            provider_readiness
        );
    }

    #[test]
    fn bundled_server_failure_uses_existing_error_overlay_path() {
        let message = "認識モデル kodama-ja-streaming-small が見つかりません";
        let mut controller =
            test_controller_with_failure(Settings::default(), Some(message.to_string()));

        controller.require_connection();

        assert_eq!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Error,
                committed: String::new(),
                partial: String::new(),
                error: message.to_string(),
            }
        );
    }

    fn settings_with(update: impl FnOnce(&mut Settings)) -> Settings {
        let mut settings = Settings::default();
        update(&mut settings);
        settings
    }

    #[test]
    fn server_endpoint_mode_uses_the_configured_min_silence() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 300;
        });
        let mut gate = gate_from_settings(&settings);

        assert_eq!(gate.push(1.0), Some(GateEvent::SpeechStarted));
        for _ in 0..9 {
            assert_eq!(gate.push(0.0), None);
        }
        assert_eq!(gate.push(0.0), Some(GateEvent::SpeechEnded));
    }

    fn asr_token(text: &str, is_final: bool) -> AsrToken {
        AsrToken {
            text: text.to_string(),
            start_ms: None,
            end_ms: None,
            confidence: None,
            is_final,
            speaker: None,
            language: None,
            translation_status: None,
            source_language: None,
        }
    }

    fn streaming_controller(
        settings: Settings,
    ) -> (Controller, crossbeam_channel::Receiver<AsrCommand>) {
        let mut controller = test_controller(settings);
        let (to_asr, asr_commands) = crossbeam_channel::unbounded();
        controller.to_asr = Some(to_asr);
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        assert!(controller.session.apply(SessionInput::Connected));
        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);
        (controller, asr_commands)
    }

    fn end_speech(controller: &mut Controller) {
        assert_eq!(controller.gate.push(0.0), Some(GateEvent::SpeechEnded));
        controller.handle_gate_event(GateEvent::SpeechEnded);
    }

    fn age_server_turn(controller: &mut Controller, elapsed: Duration) {
        controller.server_turn = match controller.server_turn {
            ServerTurn::AwaitingFirstResponse { .. } => ServerTurn::AwaitingFirstResponse {
                since: Instant::now() - elapsed,
            },
            ServerTurn::Receiving { .. } => ServerTurn::Receiving {
                last_activity_at: Instant::now() - elapsed,
            },
            ServerTurn::Idle | ServerTurn::Completed => {
                panic!("応答待ちまたは受信中であるはず")
            }
        };
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
    fn pending_audio_limit_keeps_the_newest_frames_and_records_the_drop() {
        let mut controller = test_controller(Settings::default());
        for value in 0..=PENDING_AUDIO_LIMIT {
            controller.queue_pending_audio(vec![value as u8]);
        }

        assert_eq!(controller.pending_audio.len(), PENDING_AUDIO_LIMIT);
        assert_eq!(controller.pending_audio.first(), Some(&vec![1]));
        assert_eq!(
            controller.pending_audio.last(),
            Some(&vec![PENDING_AUDIO_LIMIT as u8])
        );
        assert_eq!(controller.pending_audio_dropped_frames, 1);
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
            None,
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
            None,
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

        controller.commit_segment(Some("貼り付けテキスト".to_string()));
        let (committed, partial) = committed_overlay(&controller);
        assert_eq!(committed, "貼り付けテキスト");
        assert!(partial.is_empty());

        controller.committed_hold_until = Some(Instant::now() - Duration::from_millis(1));
        controller.check_commit_hold_timeout();
        assert_eq!(controller.overlay, OverlayView::Hidden);
    }

    #[test]
    fn notice_is_temporary_and_does_not_change_the_session_or_transcript() {
        let mut controller = test_controller(Settings::default());
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        assert!(controller.session.apply(SessionInput::Connected));

        controller.handle_asr_event(AsrEvent::Notice {
            code: "gate_blocked".to_string(),
            message: "登録した声と一致しませんでした。".to_string(),
        });

        assert_eq!(controller.session.state(), SessionState::Streaming);
        assert!(controller.transcript.is_empty());
        assert!(controller.pending_commit.is_empty());
        assert!(controller.last_commit.is_none());
        assert!(matches!(
            &controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Notice,
                error,
                ..
            } if error == "登録した声と一致しませんでした。"
        ));
        assert!(controller.overlay_notice_until.is_some());

        controller.overlay_notice_until = Some(Instant::now() - OVERLAY_NOTICE_DURATION);
        controller.check_overlay_timeout();

        assert_eq!(controller.overlay, OverlayView::Hidden);
        assert_eq!(controller.session.state(), SessionState::Streaming);
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
        controller.transcript.replace_partial("途中の文字");

        controller.refresh_overlay();

        assert_eq!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Recognizing,
                committed: String::new(),
                partial: "途中の文字".to_string(),
                error: String::new(),
            }
        );
    }

    #[test]
    fn server_mode_hides_empty_streaming_overlay_after_vad() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let mut controller = test_controller(settings);
        let (to_asr, _asr_commands) = crossbeam_channel::unbounded();
        controller.to_asr = Some(to_asr);
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
    fn server_mode_sends_nothing_when_local_vad_thinks_speech_ended() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);

        // 端末の VAD が決めてよいのは「話し始め」だけである。発話が
        // 終わったかどうかは ASR サーバーが判断するので、ここで何かを
        // 送ってはいけない。送ると端末が判断を肩代わりすることになる。
        assert!(asr_commands.try_recv().is_err());

        controller.handle_gate_event(GateEvent::SpeechEnded);
        assert!(asr_commands.try_recv().is_err());
    }

    #[test]
    fn ordinary_two_point_five_eight_second_response_never_shows_waiting() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        age_server_turn(&mut controller, Duration::from_millis(2_580));
        controller.check_server_turn_overlay();

        assert_eq!(controller.overlay, OverlayView::Hidden);
    }

    #[test]
    fn awaiting_first_response_never_becomes_starting_server() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        age_server_turn(&mut controller, SERVER_RESPONSE_WAITING_OVERLAY_DELAY);
        controller.check_server_turn_overlay();
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WaitingForResponse,
                ..
            }
        ));

        age_server_turn(&mut controller, Duration::from_secs(10));
        controller.check_server_turn_overlay();
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WaitingForResponse,
                ..
            }
        ));
    }

    #[test]
    fn awaiting_first_response_is_not_rewound_by_more_local_speech() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        age_server_turn(&mut controller, SERVER_RESPONSE_WAITING_OVERLAY_DELAY);
        let ServerTurn::AwaitingFirstResponse {
            since: first_started_at,
        } = controller.server_turn
        else {
            panic!("最初の応答待ちであるはず");
        };
        controller.check_server_turn_overlay();

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);
        assert_eq!(
            controller.server_turn,
            ServerTurn::AwaitingFirstResponse {
                since: first_started_at
            }
        );
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WaitingForResponse,
                ..
            }
        ));

        end_speech(&mut controller);
        // 積み増さない。開始時刻は「待っていない状態から最初に送った時刻」を保つ。
        assert_eq!(
            controller.server_turn,
            ServerTurn::AwaitingFirstResponse {
                since: first_started_at
            }
        );
    }

    #[test]
    fn one_server_turn_does_not_grow_with_local_vad_segments() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.auto_paste = false;
            settings.commit_hold_ms = 0;
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        let first_turn = controller.server_turn;

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);
        end_speech(&mut controller);
        let second_turn = controller.server_turn;

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);
        controller.handle_asr_event(AsrEvent::FinalText(vec![asr_token("古い結果", true)]));
        controller.handle_asr_event(AsrEvent::Endpoint);

        // 2026-08-25: 待ちは常に一つの turn である。端末の VAD が無音を検知しても
        // サーバーは終話と判断したときだけ返すので、複数回の検知が 1 つの応答に
        // まとめられる。1 対 1 に数えると行列が伸び続け、先頭が古いまま残って
        // 待ち表示が消えなくなる(実機で発生)。したがって 2 回目以降も同じ相を保ち、
        // 応答が来れば Completed にする。
        assert_eq!(first_turn, second_turn);
        assert_eq!(controller.server_turn, ServerTurn::Completed);
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Recognizing,
                ..
            }
        ));
    }

    #[test]
    fn awaiting_first_response_shows_waiting_after_its_display_delay() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        age_server_turn(&mut controller, SERVER_RESPONSE_WAITING_OVERLAY_DELAY);
        controller.check_server_turn_overlay();

        assert_eq!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WaitingForResponse,
                committed: String::new(),
                partial: String::new(),
                error: String::new(),
            }
        );
    }

    #[test]
    fn connecting_alone_can_derive_starting_server() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let mut controller = test_controller(settings);
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        controller.connecting_started_at = Some(Instant::now() - CONNECTING_STARTING_OVERLAY_DELAY);
        controller.refresh_overlay();

        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::StartingServer,
                ..
            }
        ));
    }

    #[test]
    fn awaiting_first_response_keeps_its_start_time_during_the_next_speech() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        let started_at = Instant::now() - SERVER_RESPONSE_WAITING_OVERLAY_DELAY;
        controller.server_turn = ServerTurn::AwaitingFirstResponse { since: started_at };
        controller.check_server_turn_overlay();

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);

        assert_eq!(
            controller.server_turn,
            ServerTurn::AwaitingFirstResponse { since: started_at }
        );
        assert_eq!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WaitingForResponse,
                committed: String::new(),
                partial: String::new(),
                error: String::new(),
            }
        );

        end_speech(&mut controller);
        assert_eq!(
            controller.server_turn,
            ServerTurn::AwaitingFirstResponse { since: started_at }
        );
    }

    #[test]
    fn streaming_wait_never_derives_starting_server() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        let started_at = Instant::now() - Duration::from_secs(10);
        controller.server_turn = ServerTurn::AwaitingFirstResponse { since: started_at };
        controller.check_server_turn_overlay();

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);
        end_speech(&mut controller);
        controller.check_server_turn_overlay();

        assert_eq!(
            controller.server_turn,
            ServerTurn::AwaitingFirstResponse { since: started_at }
        );
        assert!(!matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::StartingServer,
                ..
            }
        ));
    }

    #[test]
    fn partial_response_enters_receiving_and_resets_final_wait() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        age_server_turn(&mut controller, Duration::from_secs(10));
        controller.handle_asr_event(AsrEvent::PartialText(vec![asr_token("途中結果", false)]));
        let ServerTurn::Receiving { last_activity_at } = controller.server_turn else {
            panic!("途中結果は turn を閉じず Receiving へ進めるはず");
        };
        assert!(last_activity_at.elapsed() < Duration::from_secs(1));

        age_server_turn(&mut controller, Duration::from_secs(10));
        controller.check_server_turn_overlay();

        assert!(matches!(
            controller.server_turn,
            ServerTurn::Receiving { .. }
        ));
        assert!(!matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::StartingServer,
                ..
            }
        ));
    }

    #[test]
    fn response_wait_phases_are_scoped_to_streaming() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        assert_eq!(controller.session.state(), SessionState::Streaming);
        assert!(matches!(
            controller.server_turn,
            ServerTurn::AwaitingFirstResponse { .. }
        ));

        controller.handle_asr_event(AsrEvent::PartialText(vec![asr_token("途中", false)]));
        assert_eq!(controller.session.state(), SessionState::Streaming);
        assert!(matches!(
            controller.server_turn,
            ServerTurn::Receiving { .. }
        ));

        controller.handle_asr_event(AsrEvent::Failed(AsrError::ClosedEarly));
        assert_eq!(controller.session.state(), SessionState::Listening);
        assert_eq!(controller.server_turn, ServerTurn::Idle);

        controller.observe_server_activity(ServerActivity::Response, "test outside streaming");
        assert_eq!(controller.server_turn, ServerTurn::Idle);
    }

    #[test]
    fn server_response_before_wait_delay_never_shows_a_waiting_overlay() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        controller.handle_asr_event(AsrEvent::Endpoint);

        assert_eq!(controller.server_turn, ServerTurn::Completed);
        assert_eq!(controller.overlay, OverlayView::Hidden);
    }

    #[test]
    fn server_response_hides_waiting_overlay_after_transcript_commits() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.auto_paste = false;
            settings.commit_hold_ms = 0;
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        age_server_turn(&mut controller, SERVER_RESPONSE_WAITING_OVERLAY_DELAY);
        controller.check_server_turn_overlay();

        controller.handle_asr_event(AsrEvent::FinalText(vec![asr_token("確定結果", true)]));
        assert!(matches!(
            controller.server_turn,
            ServerTurn::Receiving { .. }
        ));
        controller.handle_asr_event(AsrEvent::Endpoint);

        assert_eq!(controller.server_turn, ServerTurn::Completed);
        assert_eq!(controller.overlay, OverlayView::Hidden);
        assert_eq!(
            controller
                .last_commit
                .as_ref()
                .map(|(text, _)| text.as_str()),
            Some("確定結果")
        );
    }

    #[test]
    fn fast_server_response_never_shows_a_waiting_overlay() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        age_server_turn(
            &mut controller,
            SERVER_RESPONSE_WAITING_OVERLAY_DELAY - Duration::from_millis(100),
        );
        controller.check_server_turn_overlay();

        assert_eq!(controller.overlay, OverlayView::Hidden);
    }

    #[test]
    fn server_response_notice_replaces_the_waiting_overlay() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        age_server_turn(&mut controller, SERVER_RESPONSE_WAITING_OVERLAY_DELAY);
        controller.check_server_turn_overlay();
        controller.handle_asr_event(AsrEvent::Notice {
            code: "gate_blocked".to_string(),
            message: "登録した声と一致しませんでした。".to_string(),
        });

        assert_eq!(controller.server_turn, ServerTurn::Completed);
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Notice,
                ..
            }
        ));
    }

    #[test]
    fn server_response_error_replaces_the_waiting_overlay() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        age_server_turn(&mut controller, SERVER_RESPONSE_WAITING_OVERLAY_DELAY);
        controller.check_server_turn_overlay();
        controller.handle_asr_event(AsrEvent::Failed(AsrError::Server {
            code: 503,
            error_type: "unavailable".to_string(),
            message: "starting".to_string(),
            request_id: None,
        }));

        assert_eq!(controller.server_turn, ServerTurn::Idle);
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Error,
                ..
            }
        ));
    }

    #[test]
    fn policy_close_shows_the_server_reason_in_the_overlay() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);

        controller.handle_asr_event(AsrEvent::Closed {
            code: Some(POLICY_VIOLATION_CLOSE_CODE),
            reason: "not allowed".to_string(),
        });

        assert_eq!(controller.session.state(), SessionState::Failed);
        assert_eq!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Error,
                committed: String::new(),
                partial: String::new(),
                error: "not allowed".to_string(),
            }
        );
    }

    #[test]
    fn server_mode_pauses_audio_after_endpoint_and_replays_preroll_on_next_speech() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);

        controller.handle_asr_event(AsrEvent::Endpoint);
        assert!(controller.server_audio_paused);

        controller.handle_vad_samples(&[101, -202]);
        assert!(asr_commands.try_recv().is_err());

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);

        assert!(!controller.server_audio_paused);
        match asr_commands.try_recv() {
            Ok(AsrCommand::Audio(bytes)) => {
                assert_eq!(bytes, vec![101, 0, 54, 255]);
            }
            _ => panic!("next speech should resume audio with the saved preroll"),
        }
        assert!(asr_commands.try_recv().is_err());
    }

    #[test]
    fn server_endpoint_received_during_new_speech_does_not_pause_audio() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);
        assert!(controller.gate.is_speaking());

        controller.handle_asr_event(AsrEvent::Endpoint);

        assert!(!controller.server_audio_paused);
    }

    /// `<end>` の時点で VAD が喋っていると送信を止め損ねる。止め損ねたまま
    /// にすると、以後 VAD を通さない音がサーバーへ流れ続け、背景音が新しい
    /// 発話として書き起こされる。VAD が黙った時点で止まること。
    #[test]
    fn server_endpoint_during_speech_pauses_audio_once_the_gate_goes_quiet() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);
        assert!(controller.gate.is_speaking());

        controller.handle_asr_event(AsrEvent::Endpoint);
        assert!(!controller.server_audio_paused);
        assert_eq!(controller.server_turn, ServerTurn::Completed);

        end_speech(&mut controller);
        assert!(controller.server_audio_paused, "VAD が黙ったら送信を止める");
        assert_eq!(
            controller.server_turn,
            ServerTurn::Completed,
            "server response 後の SpeechEnded は新しい待ちを作らない"
        );

        while asr_commands.try_recv().is_ok() {}
        controller.handle_vad_samples(&[101, -202]);
        assert!(
            asr_commands.try_recv().is_err(),
            "止めた後は背景音を送らない"
        );
    }

    #[test]
    fn idle_close_never_fires_while_the_gate_is_speaking() {
        let now = Instant::now();
        let old_endpoint = Some(now - Duration::from_secs(16));

        assert!(!idle_close_is_due(
            SessionState::Streaming,
            true,
            ServerTurn::Idle,
            old_endpoint,
            15,
            now
        ));
        assert!(idle_close_is_due(
            SessionState::Streaming,
            false,
            ServerTurn::Idle,
            old_endpoint,
            15,
            now
        ));
    }

    #[test]
    fn response_wait_only_blocks_idle_close_until_its_turn_timeout() {
        // 背後のコールドスタートは 30〜60 秒かかる。応答を待っている間に閉じると、
        // 起きる前に諦めることになり、その発話は永久に返らない。
        let now = Instant::now();
        let old_endpoint = Some(now - SERVER_FIRST_RESPONSE_TIMEOUT - Duration::from_secs(1));

        assert!(!idle_close_is_due(
            SessionState::Streaming,
            false,
            ServerTurn::AwaitingFirstResponse { since: now },
            old_endpoint,
            15,
            now
        ));
        assert!(idle_close_is_due(
            SessionState::Streaming,
            false,
            ServerTurn::AwaitingFirstResponse {
                since: now - SERVER_FIRST_RESPONSE_TIMEOUT,
            },
            old_endpoint,
            15,
            now
        ));
        assert!(idle_close_is_due(
            SessionState::Streaming,
            false,
            ServerTurn::Receiving {
                last_activity_at: now - SERVER_FINAL_RESPONSE_TIMEOUT,
            },
            old_endpoint,
            15,
            now
        ));
    }

    #[test]
    fn client_mode_shows_finalizing_after_speech_until_the_result_arrives() {
        // 話し終えてから結果が返るまでオーバーレイを隠すと、認識が止まった
        // ように見える。この区間は「文字にしています」を出し続ける。
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
        assert!(asr_commands.try_recv().is_err());
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
    fn partial_after_finalize_is_ignored_until_next_speech() {
        let settings = settings_with(|settings| {
            settings.auto_paste = false;
            settings.commit_hold_ms = 0;
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);
        end_speech(&mut controller);
        assert!(matches!(asr_commands.try_recv(), Ok(AsrCommand::Finalize)));

        controller.handle_asr_event(AsrEvent::FinalText(vec![asr_token("確定", true)]));
        controller.handle_asr_event(AsrEvent::FinalizeDone);

        assert!(controller.transcript.is_empty());
        assert_eq!(controller.overlay, OverlayView::Hidden);
        assert_eq!(
            controller
                .last_commit
                .as_ref()
                .map(|(text, _)| text.as_str()),
            Some("確定")
        );

        controller.handle_asr_event(AsrEvent::PartialText(vec![asr_token("あ", false)]));
        assert!(controller.transcript.is_empty());
        assert_eq!(controller.overlay, OverlayView::Hidden);

        controller.handle_asr_event(AsrEvent::FinalText(vec![asr_token("遅延", true)]));
        assert!(controller.transcript.is_empty());
        assert_eq!(controller.overlay, OverlayView::Hidden);
    }

    #[test]
    fn partial_after_next_speech_started_is_shown() {
        let settings = settings_with(|settings| {
            settings.auto_paste = false;
            settings.commit_hold_ms = 0;
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);
        end_speech(&mut controller);
        controller.handle_asr_event(AsrEvent::FinalizeDone);
        assert_eq!(controller.overlay, OverlayView::Hidden);

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);
        controller.handle_asr_event(AsrEvent::PartialText(vec![asr_token("次の", false)]));

        assert_eq!(controller.transcript.partial(), "次の");
        assert_eq!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Recognizing,
                committed: String::new(),
                partial: "次の".to_string(),
                error: String::new(),
            }
        );
    }

    #[test]
    fn server_endpoint_mode_keeps_showing_partials() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.auto_paste = false;
            settings.commit_hold_ms = 0;
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);
        end_speech(&mut controller);

        assert!(!controller.client_finalize_sent);
        controller.handle_asr_event(AsrEvent::PartialText(vec![asr_token("サーバー", false)]));

        assert_eq!(controller.transcript.partial(), "サーバー");
        assert_eq!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Recognizing,
                committed: String::new(),
                partial: "サーバー".to_string(),
                error: String::new(),
            }
        );
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
        controller.transcript.push_final("貼り付けテキスト");
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

    #[test]
    fn loopback_route_detection_accepts_local_urls() {
        for url in [
            "ws://127.0.0.1:8770/asr/v1",
            "ws://localhost:8770/",
            "ws://[::1]:8770/",
        ] {
            let parsed = Url::parse(url).expect("test URL should parse");
            assert!(is_loopback_host(parsed.host_str().unwrap()), "{url}");
        }
    }

    #[test]
    fn loopback_route_detection_rejects_remote_urls() {
        let parsed = Url::parse("wss://asr.example.com/ws/asr").expect("test URL should parse");
        assert!(!is_loopback_host(parsed.host_str().unwrap()));
    }
}

#[cfg(test)]
mod warmup_on_settings_change_tests {
    use super::{WarmupReason, WARMUP_ACTIVE_WINDOW, WARMUP_IDLE_THRESHOLD};

    /// **使っていない機械で GPU を起こし続けない。**
    ///
    /// 暖機は 60 秒ごとに打つので、止めないとアプリを開いているだけで
    /// 向こうの GPU が一日中起きたままになる。サーバーレスで動かしている
    /// 以上、使っていないなら 0 台に落ちなければ意味がない。実際に L4 が
    /// 2 台、使っていないほうも含めて起き続けた。
    #[test]
    fn the_warm_window_is_longer_than_the_gap_it_covers() {
        assert!(
            WARMUP_ACTIVE_WINDOW > WARMUP_IDLE_THRESHOLD,
            "会話の合間より短いと、話している最中に暖機が止まる"
        );
        // 際限なく起こし続けないこと。長すぎる窓は「止めない」のと同じ。
        assert!(
            WARMUP_ACTIVE_WINDOW <= std::time::Duration::from_secs(30 * 60),
            "窓が長すぎると、使っていない時間まで GPU を起こし続ける"
        );
    }

    /// 切り替えて保存した直後に暖機を打つ理由が、ログから追えること。
    #[test]
    fn the_reason_has_its_own_name() {
        assert_eq!(WarmupReason::SettingsChanged.as_str(), "settings_changed");
        // 既存の理由と混ざらない。混ざると「なぜ暖めたか」が読めなくなる。
        assert_ne!(
            WarmupReason::SettingsChanged.as_str(),
            WarmupReason::Idle.as_str()
        );
        assert_ne!(
            WarmupReason::SettingsChanged.as_str(),
            WarmupReason::Startup.as_str()
        );
    }
}
