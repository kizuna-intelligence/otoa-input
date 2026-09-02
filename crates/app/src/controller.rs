use crate::bundled_server;
use crate::settings::Settings;
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use otoa_input_core::{Account, ConnectionControl};
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

/// 接続ができるまで抱えておく音声の上限。**数ではなく長さで持つ。**
///
/// フレーム数で 100 に切っていたときは、VAD の 1 フレームが 32ms なので
/// 約 3.2 秒しか入らなかった。**1 発話ぶんにも足りない。** 接続が少しでも
/// 遅れると発話の頭から溢れ、古い順に黙って捨てるので、利用者からは
/// 「喋っても何も起きない」に見える。遠隔に 4 秒の遅延を掛けて再現した
/// ── 4 発話すべてが 1 文字も返らなかった（2026-08-31）。
///
/// 16kHz の s16 で 60 秒ぶんでも 1.9MB しかない。音声を失うより小さい。
const PENDING_AUDIO_MAX_BYTES: usize = 16_000 * 2 * 60;
const CONTROLLER_TICK: Duration = Duration::from_millis(100);
const TEXT_UI_MIN_INTERVAL: Duration = Duration::from_millis(30);
const AUDIO_PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(1);
const OVERLAY_ERROR_DURATION: Duration = Duration::from_secs(8);
const OVERLAY_NOTICE_DURATION: Duration = Duration::from_secs(3);
/// 通常応答の実機中央値は 2.58 秒。通常時に待機表示を出さない余裕として 4.0 秒にする。
const SERVER_RESPONSE_WAITING_OVERLAY_DELAY: Duration = Duration::from_secs(4);
/// コールドスタートは実測 16〜64 秒。早期に起動待ちを伝えるため、10.0 秒で切り替える。
const SERVER_RESPONSE_STARTING_OVERLAY_DELAY: Duration = Duration::from_secs(10);
const FAILED_RETRY_INITIAL: Duration = Duration::from_secs(5);
const FAILED_RETRY_MAX: Duration = Duration::from_secs(30);
/// enrollment のリモート失敗は、100 ms tick ごとに再試行せず最低 5 秒待つ。
const ENROLL_RETRY_INITIAL: Duration = Duration::from_secs(5);
const ENROLL_RETRY_MAX: Duration = Duration::from_secs(60);
/// 認識器が寝るまでの時間に合わせた既定。これ以上 ASR の成功応答が無ければ、
/// 次の発話の前に enrollment を送り、認識器が起きるまで待つ。
///
/// **サーバーが `warmup_after_secs` を名乗ればそちらに従う。** 向こうの配備が
/// 変わると、ここの数字は黙って合わなくなる。
const WARMUP_IDLE_THRESHOLD: Duration = Duration::from_secs(60);
/// 暖機を待って発話を保留するのは、これだけまで。
///
/// **遠隔が遅い日に、喋ったぶんを全部飲ませない。** 登録は普段 0.3 秒で終わる。
/// それが返らないときに待ち続けると、その間の発話は保留に積まれるだけで
/// 1 つも送られない（`ENROLL_TIMEOUT` は 180 秒ある）。実際に、起動直後の
/// 暖機が返らず 4 発話が続けて消えた。
///
/// 接続中の音声バッファと同じ 60 秒まで待つ。
///
/// gateway は WebSocket へ昇格する前に利用枠を登録する。その登録が 25 秒かかった
/// 実例があり、従来の 10 秒では、接続自体は成功する直前なのに保留した最初の発話を
/// 全て捨てていた。バッファが保持できる間は、時間だけを理由に捨てない。
#[allow(dead_code)]
const CONNECTING_TIMEOUT: Duration = Duration::from_secs(60);
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

/// 暖機に待たされて始まった発話。
struct DelayedTurn {
    /// 暖機を始めた時刻。準備中の表示はここから地続きにする。
    began_at: Instant,
    /// 端末の VAD が終話まで見届けていたか。
    speech_ended: bool,
}

/// 走っている暖機。生まれるときも消えるときも一式で動く。
struct WarmupJob {
    started_at: Instant,
    reason: WarmupReason,
    rx: Receiver<EnrollmentWarmupResult>,
    thread: Option<thread::JoinHandle<()>>,
}

impl WarmupJob {
    /// 試験用に、走っていることにするだけの job を作る。
    #[cfg(test)]
    /// 走ったまま返らない暖機。**受け口を never にする。** 送信端を落とすと
    /// 切断として届き、走っているはずの暖機がその場で終わってしまう。
    fn for_test(started_at: Instant) -> Self {
        let rx = crossbeam_channel::never();
        Self {
            started_at,
            reason: WarmupReason::BeforeSpeech,
            rx,
            thread: None,
        }
    }
}

/// ASR への接続一式。生まれるときも消えるときも 3 つ揃っている。
pub(crate) struct AsrTransport {
    to_asr: Sender<AsrCommand>,
    events: Receiver<AsrEvent>,
    thread: thread::JoinHandle<()>,
    control_stop: Option<Sender<()>>,
    control_events: Option<Receiver<ConnectionControlEvent>>,
    control_thread: Option<thread::JoinHandle<()>>,
}

enum ConnectionControlEvent {
    Superseded,
    Unavailable,
}

impl AsrTransport {
    /// 試験用に、送り口だけを差し替えた一式を作る。受け口とスレッドは
    /// 何もしないものを置く。**本体では使わない。**
    #[cfg(test)]
    fn for_test(to_asr: Sender<AsrCommand>) -> Self {
        let (_events_tx, events) = crossbeam_channel::unbounded();
        Self {
            to_asr,
            events,
            thread: thread::spawn(|| {}),
            control_stop: None,
            control_events: None,
            control_thread: None,
        }
    }
}

fn spawn_connection_control(
    control: ConnectionControl,
) -> (
    Sender<()>,
    Receiver<ConnectionControlEvent>,
    thread::JoinHandle<()>,
) {
    let (stop_tx, stop_rx) = crossbeam_channel::bounded(1);
    let (event_tx, event_rx) = crossbeam_channel::bounded(1);
    let thread = thread::Builder::new()
        .name("otoa-connection-control".to_string())
        .spawn(move || {
            let client = match reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
            {
                Ok(client) => client,
                Err(_) => {
                    let _ = event_tx.send(ConnectionControlEvent::Unavailable);
                    return;
                }
            };
            let interval = Duration::from_millis(control.poll_interval_ms.max(100));
            let mut consecutive_failures = 0_u8;
            loop {
                match stop_rx.recv_timeout(interval) {
                    Ok(()) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        complete_connection_control(&client, &control);
                        return;
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                }
                let mut request = client.get(&control.status_url);
                for (name, value) in &control.headers {
                    request = request.header(name, value);
                }
                let state = request
                    .send()
                    .and_then(|response| response.error_for_status())
                    .and_then(|response| response.json::<serde_json::Value>())
                    .map(|body| {
                        body.get("state")
                            .and_then(|value| value.as_str())
                            .map(str::to_owned)
                    });
                match state.as_ref().map(|state| state.as_deref()) {
                    Ok(Some("ready")) => consecutive_failures = 0,
                    Ok(Some("superseded")) => {
                        let _ = event_tx.send(ConnectionControlEvent::Superseded);
                        let _ = stop_rx.recv();
                        complete_connection_control(&client, &control);
                        return;
                    }
                    Ok(_) => {
                        let _ = event_tx.send(ConnectionControlEvent::Unavailable);
                        let _ = stop_rx.recv();
                        complete_connection_control(&client, &control);
                        return;
                    }
                    Err(error) => {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        tracing::warn!(
                            %error,
                            consecutive_failures,
                            "connection status could not be confirmed"
                        );
                        if consecutive_failures >= 2 {
                            let _ = event_tx.send(ConnectionControlEvent::Unavailable);
                            let _ = stop_rx.recv();
                            complete_connection_control(&client, &control);
                            return;
                        }
                    }
                }
            }
        })
        .expect("connection control thread must start");
    (stop_tx, event_rx, thread)
}

fn complete_connection_control(client: &reqwest::blocking::Client, control: &ConnectionControl) {
    for attempt in 0..4 {
        let mut request = client.post(&control.complete_url);
        for (name, value) in &control.headers {
            request = request.header(name, value);
        }
        match request.send() {
            Ok(response) if response.status() == reqwest::StatusCode::NO_CONTENT => return,
            Ok(response) if response.status() == reqwest::StatusCode::ACCEPTED && attempt < 3 => {
                thread::sleep(Duration::from_millis(500));
            }
            Ok(response) => {
                tracing::warn!(
                    status = %response.status(),
                    "connection completion could not be settled"
                );
                return;
            }
            Err(error) => {
                tracing::warn!(%error, "connection completion could not be recorded");
                return;
            }
        }
    }
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
    gate: GateState,
    warmup: Option<Instant>,
    /// 第1段では旧来どおり一件だけ。第3段で FIFO として有効化する。
    /// サーバーへ音声を送ってから、**まだ何も返ってきていない**時間の起点。
    ///
    /// 行列ではない。実装も空のときしか積んでいなかったので常に 0/1 件で、
    /// `epoch` は記録に出すだけで応答との照合に一度も使っていなかった
    /// （サーバーの応答に epoch も request id も無いので、持っても対応が付かない）。
    ///
    /// **サーバーから何か届いたら必ず解ける。** 途中結果でも解ける。以前は
    /// `PartialText` で解かなかったため、応答が届いているのに古い起点が居座り、
    /// 10 秒を超えて「サーバーを起動しています」に変わっていた。喋り続けている
    /// のに出る、という形で実際に起きた（epoch 21 が 26 秒残った）。
    pending_since: Option<Instant>,
    commit: Option<(String, Instant)>,
    notice: Option<Notice>,
    error: Option<UserError>,
    /// 途中結果は gate が Idle になった後にも届くため、gate と独立に持つ。
    partial: Option<String>,
    /// client endpoint の finalize を送ってから確定応答を受けるまで。
    finalizing: bool,
}

/// `Facts` だけからオーバーレイを導出する。副作用を持たない。
///
fn view(facts: &Facts, now: Instant) -> OverlayView {
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

    // 貼り付けを止める状態が残っているあいだは、画面も必ず「まだ話さないで
    // ください」のままにする。確定文字や通知を先に出すと、画面だけ準備済みに
    // 見えて内部では貼り付けを止める、という表現してはいけない組が生まれる。
    if facts.warmup.is_some() {
        return blank_overlay(OverlayKind::WarmingUp);
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

    if let Some(since) = facts.pending_since {
        let elapsed = now.saturating_duration_since(since);
        if elapsed >= SERVER_RESPONSE_STARTING_OVERLAY_DELAY {
            return blank_overlay(OverlayKind::StartingServer);
        }
        if elapsed >= SERVER_RESPONSE_WAITING_OVERLAY_DELAY {
            return blank_overlay(OverlayKind::WaitingForResponse);
        }
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
    /// 最初の発話の直前。登録が古いので、喋り出しを待たせて入れ直す。
    BeforeSpeech,
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
            Self::BeforeSpeech => "before_speech",
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
    /// ASR への接続一式。**3 つに分けない。** 送り口・受け口・スレッドは
    /// 必ず同時に生まれて同時に消える。別々の Option に分けていたときは、
    /// 片方だけ残った状態を型が許していた。
    pub(crate) asr: Option<AsrTransport>,
    pub(crate) to_ui: Sender<UiUpdate>,
    pub(crate) text_out: TextOutput,
    facts: Facts,
    /// 前回描画した値。表示理由は保持せず、Facts から導出した結果の重複送信を抑える。
    overlay: OverlayView,
    /// 第1・第2段のテストが直接観測している期限。製品の表示状態には使わない。
    #[cfg(test)]
    overlay_error_until: Option<Instant>,
    #[cfg(test)]
    overlay_notice_until: Option<Instant>,
    splash_started_at: Option<Instant>,
    /// enrollment worker が走っているか。
    /// 走っている暖機。**進行中かどうかは、これが有るかどうかである。**
    ///
    /// 進行中の旗・受け口・スレッド・開始時刻・理由を別々に持っていたときは、
    /// 「進行中なのに開始時刻が無い」ような組み合わせを型が許していた。実際、
    /// 片付けが開始時刻を先に消すので、保留した発話を送り直すときに本当の
    /// 開始時刻を失っていた。
    ///
    /// 取り消しはこれを落とすだけでよい。受け口ごと落ちるので、遅れて届く
    /// 結果は送り先を失って捨てられる。通し番号で照合する必要が無くなった。
    warmup: Option<WarmupJob>,
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
    /// 暖機に待たされて始まった発話。**その発話が片付くまでの間だけ生きる。**
    ///
    /// 「終話まで見届けたか」「準備中の表示をいつから出しているか」「貼らずに
    /// クリップボードへ置くか」を別々の旗で持っていたときは、消し忘れた旗が
    /// 次の無関係な発話へ漏れた。持ち主を発話ひとつにして、片付けと同時に
    /// 必ず消えるようにする。
    delayed_turn: Option<DelayedTurn>,
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
    /// endpoint_mode=server のとき、`<end>` を受けてから次の発話開始までの抑止。
    /// 第1・第2段のテストが直接観測している単一待機。製品コードは `Facts.pending`
    /// の FIFO だけを使う。
    #[cfg(test)]
    server_response_wait_started_at: Option<Instant>,
    #[cfg(test)]
    server_response_wait_overlay_visible: bool,
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
    /// `FinalText` と直後の `<end>` / `<fin>` は同じ応答フレームから分解された
    /// イベントである。前者で FIFO を進めた後、後者が次の待機を二重に pop しない
    /// ようにする。
    pending_completed_by_final_text: bool,
    /// `<end>` の直後に届く通知は同じ応答に属する。次の待機を消さないための一回限り
    /// の抑止で、次の通常応答が始まれば解除する。
    pending_completed_by_endpoint: bool,
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
    pending_commit: String,
    #[cfg(test)]
    committed_hold_until: Option<Instant>,
    last_text_ui: Option<Instant>,
    audio_capture: Option<AudioCapture>,
    pending_settings: Option<Settings>,
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
        let preroll = PreRoll::new(milliseconds_to_samples(effective_preroll_ms(&settings)));
        let splash_started_at = Instant::now();
        let mut text_out = TextOutput::new()?;
        configure_text_output(&mut text_out, &settings);
        Ok(Self {
            session: Session::new(),
            transcript: Transcript::new(),
            settings,
            pending_audio: Vec::new(),
            pending_audio_dropped_frames: 0,
            asr: None,
            to_ui,
            text_out,
            facts: Facts {
                gate: GateState::Idle,
                warmup: None,
                pending_since: None,
                commit: None,
                notice: None,
                error: None,
                partial: None,
                finalizing: false,
            },
            // 起動直後はロゴを見せる。main の既定に合わせる。
            overlay: OverlayView::Splash,
            #[cfg(test)]
            overlay_error_until: None,
            #[cfg(test)]
            overlay_notice_until: None,
            splash_started_at: Some(splash_started_at),
            warmup: None,
            warmup_retry_at: None,
            warmup_retry_delay: ENROLL_RETRY_INITIAL,
            last_successful_asr_response_at: None,
            deferred_speech: None,
            delayed_turn: None,
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
            #[cfg(test)]
            server_response_wait_started_at: None,
            #[cfg(test)]
            server_response_wait_overlay_visible: false,
            server_audio_paused: false,
            pause_when_gate_stops: false,
            finalize_pending: false,
            last_confident_speech_at: None,
            pending_completed_by_final_text: false,
            pending_completed_by_endpoint: false,
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
            pending_commit: String::new(),
            #[cfg(test)]
            committed_hold_until: None,
            last_text_ui: None,
            audio_capture: None,
            pending_settings: None,
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
            self.drain_connection_control_events();
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
        if self.warmup.is_some() {
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
        tracing::info!(target: "otoa_input", reason = reason.as_str(), "warmup: started");

        let worker = thread::Builder::new()
            .name("otoa-warmup".to_string())
            .spawn(move || {
                let outcome = provider.ensure_enrolled(&settings, EnrollReason::Warmup);
                let _ = result_tx.send(EnrollmentWarmupResult {
                    reason,
                    started_at,
                    outcome,
                });
            });

        match worker {
            Ok(worker) => {
                // **job を入れてから描く。** 表示は job から作り直されるので、
                // 先に描くと「暖機中」が即座に消える。
                self.warmup = Some(WarmupJob {
                    started_at,
                    reason,
                    rx: result_rx,
                    thread: Some(worker),
                });
                self.splash_started_at = None;
                self.refresh_overlay();
            }
            Err(error) => {
                self.finish_enrollment_warmup(EnrollmentWarmupResult {
                    reason,
                    started_at,
                    outcome: EnrollOutcome::RetryableRemote(format!(
                        "起動処理を開始できませんでした: {error}"
                    )),
                });
            }
        }
        true
    }

    fn drain_warmup_events(&mut self) {
        let Some(job) = self.warmup.as_ref() else {
            return;
        };
        let result = match job.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => EnrollmentWarmupResult {
                reason: job.reason,
                started_at: job.started_at,
                outcome: EnrollOutcome::RetryableRemote(
                    "起動処理が予期せず終了しました".to_string(),
                ),
            },
        };
        // **結果を扱う前に job を落とさない。** 開始時刻は保留した発話を
        // 送り直すときにも使う。先に消すと本当の開始時刻を失う。
        self.finish_enrollment_warmup(result);
    }

    fn cancel_warmup(&mut self) {
        // reqwest の blocking request は途中で安全に取り消せない。終了を待つと
        // 最大 timeout までアプリを閉じられなくなるため、JoinHandle を外して
        // プロセス終了に任せる。job ごと落とせば受け口も消えるので、遅れて
        // 届く結果は送り先を失って捨てられる。
        if let Some(mut job) = self.warmup.take() {
            job.thread.take();
        }
        self.facts.warmup = None;
        self.clear_deferred_warmup_speech();
    }

    fn finish_enrollment_warmup(&mut self, result: EnrollmentWarmupResult) {
        let was_warming = self.is_warming_overlay();
        if let Some(mut job) = self.warmup.take() {
            if let Some(thread) = job.thread.take() {
                let _ = thread.join();
            }
        }
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
                    self.resume_deferred_warmup_speech(result.started_at);
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
                    self.resume_deferred_warmup_speech(result.started_at);
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
        self.warmup = None;
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
        self.delayed_turn = None;
    }

    /// 保留していた発話で接続を続ける。保留が無ければ何もしない。
    fn resume_deferred_warmup_speech(&mut self, warmup_started_at: Instant) {
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
        // 暖機から始めた発話であることは、認識接続まで準備できる間だけ持つ。
        // 発話終了より先に接続できれば通常発話へ戻し、発話が先に終わった場合だけ
        // 結果を貼らずクリップボードへ置く。暖機処理だけが短く終わっても、冷えた
        // 認識接続が数十秒かかる実例があるため、ここではまだ決めきらない。
        self.delayed_turn = Some(DelayedTurn {
            began_at: warmup_started_at,
            speech_ended,
        });
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
        // **待受に入っただけでは、向こうを起こさない。** 認識器はゼロまで
        // 落ちる配備なので、起こせばその間ずっと課金される。喋っていない人の
        // ために起こし続ける理由は無い。登録は最初の発話のときにまとめてやり、
        // 待たせているあいだは「準備中」を出す。
    }

    fn disable_listening(&mut self) {
        // blocking enrollment は止められない場合がある。receiver を外し epoch を
        // 進めることで、Disabled になった後の結果を必ず捨てる。
        self.cancel_warmup();
        self.suspend_vad();
        self.audio_capture.take();
        self.gate.reset();
        self.preroll.clear();
        self.level_clip_window.clear();
        self.clear_pending_waits("listening disabled");

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
        // 開始はこのフレームを送れる状態にしてから、終了はこのフレームを
        // 送り切ってから扱う。同じ順序で処理すると、どちらか一方の端が必ず
        // 区切りの外へ出る。
        for event in events.iter().copied() {
            if event == GateEvent::SpeechStarted {
                self.handle_gate_event(event);
            }
        }
        self.handle_vad_samples(&samples);
        for event in events {
            if event == GateEvent::SpeechEnded {
                self.handle_gate_event(event);
            }
        }
    }

    fn handle_gate_event(&mut self, event: GateEvent) {
        match event {
            GateEvent::SpeechStarted => {
                tracing::debug!(target: "otoa_input", "gate: speech started");
                self.facts.gate = GateState::Speaking;
                let previous_turn_finalized = self.client_finalize_sent;
                self.client_finalize_sent = false;
                // 利用者が次を喋り始めていても、届いていない応答は前の発話の
                // ものでありうる。サーバーが実際に答えるか、セッションが終わる
                // まで、その待ちを消さない。
                self.log_server_response_wait_kept("next speech started");
                self.clear_commit_hold();
                self.clear_overlay_notice();
                if self.warmup.is_some() {
                    // 暖機と同時に話し始めた場合は結果を待つ。終わったら
                    // この音声で接続を続ける。
                    //
                    // **既に保留があるなら上書きしない。** 上書きすると、
                    // 暖機が長引いたときに最後の 1 発話しか残らない。
                    // 続けて喋ったぶんは同じ保留へ足し、まとめて送る。
                    let preroll = self.preroll.take();
                    let deferred = self
                        .deferred_speech
                        .get_or_insert_with(DeferredSpeech::default);
                    if deferred.preroll.is_empty() {
                        deferred.preroll = preroll;
                    }
                    deferred.ended = false;
                    tracing::debug!(target: "otoa_input", "speech deferred while warmup is running");
                    return;
                }
                if self.session.state() == SessionState::Listening && self.warmup_is_due() {
                    self.deferred_speech = Some(DeferredSpeech {
                        preroll: self.preroll.take(),
                        ..DeferredSpeech::default()
                    });
                    if self.start_warmup(WarmupReason::BeforeSpeech) {
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
                        if !self.send_speech_start() {
                            return;
                        }
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
                    SessionState::Connecting if previous_turn_finalized => {
                        // WebSocket の接続確立前でも、前の発話の Audio と Finalize は
                        // ASR スレッドの送信キューに入っている。次の発話境界も同じ
                        // キューへ置けば、接続後に二つの声が一つの話者照合区間へ
                        // 混ざらない。
                        if !self.send_speech_start() {
                            return;
                        }
                        self.server_audio_paused = false;
                        self.pause_when_gate_stops = false;
                        let preroll = self.preroll.take();
                        if !preroll.is_empty() {
                            self.queue_pending_audio(samples_to_bytes(&preroll));
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
                if let Some(turn) = self.delayed_turn.as_mut() {
                    turn.speech_ended = true;
                }
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
    ///
    /// **ここはコントローラのスレッドを止めて往復する。** 直前に暖機が通って
    /// いるなら、同じ登録をもう一度送っても結果は変わらない。遠隔が遅い日は
    /// この往復のぶんだけ接続が遅れ、1 発話目が間に合わなくなる（片道 2 秒で
    /// 実測 16 秒。4 発話中 1 発話が落ちた）。
    ///
    /// 省いてよいのは「直前の登録が通っている」ときだけである。
    /// `warmup_is_due` は「最近使ったか」も含むので使えない ── 一度も喋って
    /// いない起動直後は偽になり、声を登録していない人へ何も伝えないまま
    /// 接続してしまう。
    fn ensure_enrolled_before_connection(&mut self) -> bool {
        let enrollment_is_fresh = self
            .last_successful_asr_response_at
            .is_some_and(|at| at.elapsed() < self.warmup_idle_threshold());
        if enrollment_is_fresh {
            return true;
        }
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

    /// endpoint_mode = client / both では端末 VAD の区切りを一度だけ送る。
    /// 遅延 warmup 後に Connected になった場合にも同じ処理を使う。
    fn finish_current_speech(&mut self) {
        let client_can_finalize = matches!(self.settings.endpoint_mode.as_str(), "client" | "both");
        if client_can_finalize
            && matches!(
                self.session.state(),
                SessionState::Connecting | SessionState::Streaming
            )
            && !self.client_finalize_sent
        {
            if self.settings.endpoint_mode == "both" {
                // K2 は finalize で区間を空へ戻す。応答を待つ間も音声を
                // 送り続けると、その直後の無音が次区間の先頭になり、次の
                // 発話と再び連結される。端末の区切りと同時に止め、次の
                // SpeechStarted でプリロール付きで再開する。
                self.server_audio_paused = true;
                self.pause_when_gate_stops = false;
                self.preroll.clear();
            }
            if self.session.state() == SessionState::Connecting {
                // 接続待ち中に終わった短い発話も、音声と区切りを ASR スレッドの
                // キューへ順番どおり渡す。Connected まで平坦な音声バッファへ
                // 残すと、その間に始まった次の発話と結合されてしまう。
                let pending_audio = std::mem::take(&mut self.pending_audio);
                for bytes in pending_audio {
                    if !self.send_audio_bytes(bytes) {
                        return;
                    }
                }
            }
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
            // **サーバーが先に終話を告げていたら、待ちを作らない。**
            // 端末の VAD より先に `<end>` が届くことがある。そのとき新しい
            // 待ちを作ると、対応する応答はもう来ないので永久に残り、以後
            // どれだけ喋っても経過時間だけが伸びて
            // 「サーバーを起動しています」に届いてしまう（26 秒残った実例がある）。
            if !self.pending_completed_by_endpoint {
                self.start_server_response_wait();
            }
        } else if client_can_finalize {
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
        if self.warmup.is_some() {
            // idle warmup 中は SpeechStarted が上で deferred 状態へ切り替えるまで
            // 入力を ASR に渡さない。
            return;
        }
        match self.session.state() {
            SessionState::Listening | SessionState::Failed => {
                self.preroll.push(samples);
            }
            SessionState::Connecting => {
                if self.client_finalize_sent {
                    // 発話の区切りをキューへ送った後の無音は、その発話へ足さない。
                    // 次の SpeechStarted のプリロールとしてだけ保持する。
                    self.preroll.push(samples);
                } else {
                    self.queue_pending_audio(samples_to_bytes(samples));
                }
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
        self.server_audio_paused = false;
        self.pause_when_gate_stops = false;
        let endpoint = self.provider.endpoint(&self.settings.core)?;

        let (control_stop, control_events, mut control_thread) = endpoint
            .control
            .clone()
            .map(spawn_connection_control)
            .map_or((None, None, None), |(stop, events, thread)| {
                (Some(stop), Some(events), Some(thread))
            });

        let config_key = endpoint
            .headers
            .is_empty()
            .then(|| endpoint.api_key.clone())
            .flatten();
        let mut config =
            AsrConfig::realtime_pcm16k(config_key).with_endpoint_mode(&self.settings.endpoint_mode);
        config.language_hints = self.settings.language_hints.clone();
        let (to_asr, commands) = crossbeam_channel::unbounded();
        // The command is queued before the session starts. The protocol thread
        // sends config first, then this boundary, and only then can Connected
        // release the buffered audio.
        to_asr
            .send(AsrCommand::SpeechStart)
            .map_err(|error| anyhow::anyhow!("発話開始の送信に失敗しました: {error}"))?;
        let (events, asr_events) = crossbeam_channel::unbounded();
        let asr_thread =
            match AsrSession::spawn(endpoint.url, config, endpoint.headers, commands, events) {
                Ok(thread) => thread,
                Err(error) => {
                    if let Some(stop) = control_stop.as_ref() {
                        let _ = stop.send(());
                    }
                    if let Some(thread) = control_thread.take() {
                        let _ = thread.join();
                    }
                    return Err(error.into());
                }
            };

        self.active_api_key = endpoint.api_key;
        self.asr_closing = false;
        self.asr = Some(AsrTransport {
            to_asr,
            events: asr_events,
            thread: asr_thread,
            control_stop,
            control_events,
            control_thread,
        });
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
        let mut buffered: usize = self.pending_audio.iter().map(Vec::len).sum();
        while buffered + bytes.len() > PENDING_AUDIO_MAX_BYTES && !self.pending_audio.is_empty() {
            self.pending_audio_dropped_frames += 1;
            if self.pending_audio_dropped_frames == 1
                || self.pending_audio_dropped_frames.is_multiple_of(10)
            {
                tracing::warn!(
                    target: "otoa_input",
                    pending_limit_bytes = PENDING_AUDIO_MAX_BYTES,
                    dropped_frames = self.pending_audio_dropped_frames,
                    "ASR connection is not ready; dropping oldest pending audio frame"
                );
            }
            buffered -= self.pending_audio.remove(0).len();
        }
        self.pending_audio.push(bytes);
    }

    fn send_audio(&mut self, samples: &[i16]) {
        let _ = self.send_audio_bytes(samples_to_bytes(samples));
    }

    fn send_audio_bytes(&mut self, bytes: Vec<u8>) -> bool {
        let Some(to_asr) = self.asr.as_ref().map(|asr| asr.to_asr.clone()) else {
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
        let Some(to_asr) = self.asr.as_ref().map(|asr| asr.to_asr.clone()) else {
            self.fail_runtime("音声認識セッションが利用できません".to_string());
            return false;
        };
        if let Err(error) = to_asr.send(AsrCommand::Finalize) {
            self.fail_runtime(format!("音声認識セッションの終了に失敗しました: {error}"));
            return false;
        }
        true
    }

    fn send_speech_start(&mut self) -> bool {
        let Some(to_asr) = self.asr.as_ref().map(|asr| asr.to_asr.clone()) else {
            self.fail_runtime("音声認識セッションが利用できません".to_string());
            return false;
        };
        if let Err(error) = to_asr.send(AsrCommand::SpeechStart) {
            self.fail_runtime(format!("発話開始の送信に失敗しました: {error}"));
            return false;
        }
        true
    }

    /// サーバーの応答を待ち始めた。既に待っているなら起点は動かさない。
    fn start_server_response_wait(&mut self) {
        if self.facts.pending_since.is_some() {
            return;
        }
        self.facts.pending_since = Some(Instant::now());
        self.sync_pending_wait_observer();
        tracing::debug!(target: "otoa_input", "サーバー応答待ちを始めた");
    }

    /// 発話が次へ進んでも、既に表示しているサーバー応答待ちを保持したことを記録する。
    /// gate event ごとに高頻度で出るものではないため、再表示・タイマー再始動の抑制を
    /// 実機ログから追える。
    /// 発話が次へ進んでも、既に始まっている応答待ちを引き継いだことを記録する。
    fn log_server_response_wait_kept(&self, reason: &'static str) {
        let Some(since) = self.facts.pending_since else {
            return;
        };
        let elapsed = since.elapsed();
        tracing::info!(
            target: "otoa_input",
            elapsed_ms = elapsed.as_millis() as u64,
            reason,
            "overlay: waiting kept"
        );
    }

    /// サーバーから何か届いたので待ちを解く。
    ///
    /// **途中結果でも解く。** 届いている以上、待たせている理由は無い。
    fn complete_pending_wait(&mut self, reason: &'static str) -> bool {
        let Some(since) = self.facts.pending_since.take() else {
            self.sync_pending_wait_observer();
            return false;
        };
        tracing::debug!(
            target: "otoa_input",
            elapsed_ms = since.elapsed().as_millis() as u64,
            reason,
            "サーバー応答待ちが解けた"
        );
        self.sync_pending_wait_observer();
        true
    }

    /// セッションの終了・切断時には、対応先を失った待機を取り除く。
    fn clear_pending_waits(&mut self, reason: &'static str) {
        if self.facts.pending_since.is_some() {
            tracing::debug!(target: "otoa_input", reason, "サーバー応答待ちを空にした");
        }
        self.facts.pending_since = None;
        self.pending_completed_by_final_text = false;
        self.pending_completed_by_endpoint = false;
        self.sync_pending_wait_observer();
    }

    /// 100 ms tick と既存テストからの明示的な再評価口。表示の選択は `view` だけが行う。
    fn check_server_response_wait_overlay(&mut self) {
        self.import_pending_wait_override_for_test();
        self.render_overlay();
        self.sync_pending_wait_observer();
    }

    #[cfg(test)]
    fn import_pending_wait_override_for_test(&mut self) {
        if let Some(started_at) = self.server_response_wait_started_at {
            self.facts.pending_since = Some(started_at);
        }
    }

    #[cfg(not(test))]
    fn import_pending_wait_override_for_test(&mut self) {}

    #[cfg(test)]
    fn sync_pending_wait_observer(&mut self) {
        self.server_response_wait_started_at = self.facts.pending_since;
        self.server_response_wait_overlay_visible = matches!(
            view(&self.facts, Instant::now()),
            OverlayView::Shown {
                kind: OverlayKind::WaitingForResponse | OverlayKind::StartingServer,
                ..
            }
        );
    }

    #[cfg(not(test))]
    fn sync_pending_wait_observer(&mut self) {}

    fn send_stop(&mut self) {
        if let Some(to_asr) = self.asr.as_ref().map(|asr| asr.to_asr.clone()) {
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

    fn drain_connection_control_events(&mut self) {
        let event = self
            .asr
            .as_ref()
            .and_then(|asr| asr.control_events.as_ref())
            .and_then(|events| events.try_recv().ok());
        match event {
            Some(ConnectionControlEvent::Superseded) => {
                self.disable_listening();
                self.show_persistent_overlay_error(
                    "別の端末で入力が始まったため、このマイクを停止しました。".to_string(),
                );
            }
            Some(ConnectionControlEvent::Unavailable) => {
                self.disable_listening();
                self.show_persistent_overlay_error(
                    "接続を確認できないため、入力を停止しました。もう一度お試しください。"
                        .to_string(),
                );
            }
            None => {}
        }
    }

    fn handle_vad_failure(&mut self, message: String) {
        self.suspend_vad();
        self.audio_capture.take();
        self.fail_runtime(message);
    }

    fn drain_asr_events(&mut self) {
        loop {
            let next = self.asr.as_ref().map(|asr| asr.events.try_recv());
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
                    self.asr = None;
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
                // 保留の時点で終話まで見届けていたなら、繋がった時点で確定へ進める。
                // **待たせた印は消さない。** 結果を貼らずに置くところまでが
                // この発話の扱いである。
                if self
                    .delayed_turn
                    .as_ref()
                    .is_some_and(|turn| turn.speech_ended)
                {
                    self.finish_current_speech();
                } else {
                    // 暖機をきっかけにした発話でも、話し終わる前に認識接続まで
                    // 準備できたなら、もう利用者を待たせていない。ここで通常発話へ
                    // 戻さないと、短い再暖機直後の最初の発話だけ貼られなくなる。
                    if self.delayed_turn.take().is_some() {
                        tracing::debug!(
                            target: "otoa_input",
                            "warmup-triggered speech became ready before speech ended"
                        );
                    }
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
                // FinalText はそれ自体がサーバーからの確定応答である。終話の
                // 印を待つ間、応答待ちの表示を残したままにしない。
                self.pending_completed_by_endpoint = false;
                self.pending_completed_by_final_text =
                    self.complete_pending_wait("final transcript received");
                self.transcript.push_final(&tokens_to_text(&tokens));
                self.send_text_update(true);
            }
            AsrEvent::PartialText(tokens) => {
                // `<end>` の直後の notice だけを抑止する。次の通常応答は必ず
                // partial（空でもよい）を含むので、ここで次の応答へ進める。
                self.pending_completed_by_endpoint = false;
                // **途中結果もサーバーからの応答である。** 解かないと、応答が
                // 届いているのに古い起点が居座り、10 秒を超えて
                // 「サーバーを起動しています」に変わる。喋り続けているのに出る、
                // という形で実際に起きた。
                self.complete_pending_wait("partial transcript received");
                if self.client_finalize_sent && !self.gate.is_speaking() {
                    tracing::debug!(
                        target: "otoa_input",
                        "ignored partial text after client finalize until next speech"
                    );
                    return;
                }
                let text = tokens_to_text(&tokens);
                let had_commit_hold = !text.is_empty() && self.clear_commit_hold();
                self.transcript.replace_partial(&text);
                self.send_text_update(had_commit_hold);
            }
            AsrEvent::Endpoint => {
                if self.pending_completed_by_final_text {
                    self.pending_completed_by_final_text = false;
                    self.pending_completed_by_endpoint = true;
                } else {
                    // **待ちが有ったかどうかではなく、サーバーが閉じたかを持つ。**
                    // 端末の VAD より先に `<end>` が届くと、その時点では待ちが
                    // 無いので解く相手がいない。ここで false のままにすると、
                    // 直後の終話で新しい待ちを作ってしまい、対応する応答は
                    // もう来ないので永久に残る。
                    self.complete_pending_wait("endpoint received");
                    self.pending_completed_by_endpoint = true;
                }
                self.finalize_pending = false;
                self.last_speech_endpoint_at = Some(Instant::now());
                let server_can_endpoint =
                    matches!(self.settings.endpoint_mode.as_str(), "server" | "both");
                if server_can_endpoint && self.gate.is_speaking() {
                    // 次の発話を拾っている最中なので今は止められない。VAD が
                    // 黙ったら止める。
                    self.pause_when_gate_stops = true;
                }
                if server_can_endpoint && !self.gate.is_speaking() {
                    // `<end>` を受けた後は次の SpeechStarted まで送信を止める。
                    // ただし、既に次の発話を拾っている場合は古い `<end>` の可能性
                    // があるため止めず、その発話を欠かさない。
                    self.server_audio_paused = true;
                    // ここへ来る `<end>` は前の発話の遅れた応答かもしれない。
                    // SpeechGate が次の短い発話を確認している途中では、見かけ上は
                    // まだ idle でも preroll は既にその発話の先頭を持っている。
                    // 音声送信を止めるだけにして、次発話用の有限バッファは残す。
                    tracing::debug!(target: "otoa_input", "paused server ASR audio after endpoint");
                }
                self.log_session_event("SpeechEndpoint");
                let segment = self.transcript.take_segment();
                self.commit_segment(segment);
                self.close_delayed_session_after_response(false);
                if self.facts.commit.is_none() && self.facts.error.is_none() {
                    self.clear_overlay_facts();
                    self.refresh_overlay();
                }
            }
            AsrEvent::FinalizeDone => {
                if self.pending_completed_by_final_text {
                    self.pending_completed_by_final_text = false;
                } else {
                    self.complete_pending_wait("finalize response received");
                }
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
                self.clear_pending_waits("session finished");
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
                self.complete_pending_wait("ASR WebSocket closed");
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
                if self.pending_completed_by_endpoint {
                    self.pending_completed_by_endpoint = false;
                    self.show_overlay_notice_after_response(code, message);
                } else {
                    self.show_overlay_notice(code, message);
                }
                // 通知はこの発話に文字が無いことまで決まった応答なので、端末の
                // 終話を待たずに専用接続を閉じてよい。
                self.close_delayed_session_after_response(true);
            }
            AsrEvent::Failed(error) => {
                self.complete_pending_wait("ASR error received");
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
        self.check_server_response_wait_overlay();
        if !idle_close_is_due(
            self.session.state(),
            self.gate.is_speaking(),
            self.facts.pending_since.is_some() || self.finalize_pending,
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

    /// 暖機中に受けた発話の応答を、次の通常発話と同じ接続へ混ぜない。
    ///
    /// サーバーの `<end>` には発話 ID が無く、1 回の端末発話が複数の `<end>` に
    /// 分かれることもある。最初の区切りで待たせた印を消すと、後続だけが通常の
    /// 貼り付けになる。専用接続を最後まで閉じ、`Finished` で印と表示を同時に
    /// 消せば、次の接続からは曖昧さなく通常の貼り付けへ戻せる。
    fn close_delayed_session_after_response(&mut self, force: bool) {
        let should_close = self.delayed_turn.as_ref().is_some_and(|turn| {
            (force || turn.speech_ended)
                && self.session.state() == SessionState::Streaming
                && !self.gate.is_speaking()
        });
        if !should_close {
            return;
        }

        self.send_stop();
        if self.session.apply(SessionInput::IdleTimeout) {
            self.log_session_event("DelayedTurnComplete");
            self.last_speech_endpoint_at = None;
            self.closing_started_at = Some(Instant::now());
            self.clear_overlay_facts();
            self.send_ui(UiUpdate::State(SessionState::Closing));
            self.refresh_overlay();
        }
    }

    fn commit_segment(&mut self, segment: Option<String>) {
        if let Some(segment) = segment {
            self.pending_commit.push_str(&segment);
        }

        if self.settings.auto_paste
            && !self.settings.paste_per_endpoint
            && self.session.state() != SessionState::Closing
        {
            return;
        }

        // 1 回の端末発話が複数のサーバー区切りになることがあるので、最初の
        // commit では消さない。専用接続の Finished で表示と一緒に消す。
        let was_delayed = self.delayed_turn.is_some();
        let Some(text) = take_pending(&mut self.pending_commit) else {
            return;
        };
        self.show_committed_text(text.clone());
        if !self.settings.auto_paste {
            return;
        }
        // 暖機に待たされた発話は貼らない。待っているあいだに利用者は別の窓へ
        // 移っていることがあり、そこへ貼ると取り消せない。
        let method = if was_delayed {
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
        // 接続中の待たせた印は Finished まで残す。待受中に単体で通知を出す
        // 経路だけは、対応する接続が無いのでここで片づける。
        if !matches!(
            self.session.state(),
            SessionState::Streaming | SessionState::Closing
        ) {
            self.delayed_turn = None;
        }
        self.complete_pending_wait("notice received");
        self.show_overlay_notice_after_response(code, message);
    }

    fn show_overlay_notice_after_response(&mut self, code: String, message: String) {
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
        self.clear_pending_waits("persistent error shown");
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
        self.facts.gate = if self.gate.is_speaking() {
            GateState::Speaking
        } else {
            GateState::Idle
        };
        // 走っている暖機と、暖機に待たされた発話は、どちらも同じ準備中表示に
        // する。後者は commit_segment が発話終了と一緒に消すので、表示だけ先に
        // 閉じて貼り付け禁止だけが残る状態は作れない。
        self.facts.warmup = match self.warmup.as_ref().map(|job| job.started_at) {
            Some(started_at) if self.warmup.is_some() => Some(started_at),
            _ => self.delayed_turn.as_ref().map(|turn| turn.began_at),
        };

        self.facts.partial =
            (!self.transcript.partial().is_empty()).then(|| self.transcript.partial().to_string());
        self.facts.finalizing = self.finalize_pending;
    }

    fn render_overlay(&mut self) {
        self.refresh_runtime_facts();
        self.set_overlay(view(&self.facts, Instant::now()));
        self.sync_display_deadline_observer();
        self.sync_pending_wait_observer();
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
        self.preroll = PreRoll::new(milliseconds_to_samples(effective_preroll_ms(
            &self.settings,
        )));
    }

    fn apply_pending_settings(&mut self) {
        let Some(settings) = self.pending_settings.take() else {
            return;
        };
        let was_enabled = self.settings.listening_enabled;
        configure_text_output(&mut self.text_out, &settings);
        self.settings = settings;
        self.rebuild_vad_configuration();
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
        // **待たせた印は、その発話と一緒に消える。** 消し損ねると、次の
        // 無関係な発話が貼られずクリップボードにだけ入る。
        self.delayed_turn = None;
        self.clear_pending_waits("ASR session cleaned up");
        self.finalize_pending = false;
        self.client_finalize_sent = false;
        self.server_audio_paused = false;
        self.pause_when_gate_stops = false;
        if let Some(asr) = self.asr.take() {
            let AsrTransport {
                to_asr,
                events: _,
                thread,
                control_stop,
                control_events: _,
                control_thread,
            } = asr;
            if let Some(stop) = control_stop {
                let _ = stop.send(());
            }
            if let Some(thread) = control_thread {
                let _ = thread.join();
            }
            // cleanup が接続スレッドの終了を待つ前に、そのスレッドが持つ
            // Receiver から見て送り口を必ず閉じる。設定変更は稼働中の接続を
            // ここへ直接持ってくるため、Stop/Finished を経由するとは限らない。
            // Sender を保持したまま join すると、接続スレッドは次の指示を待ち、
            // コントローラは接続スレッドを待つ相互待ちになる。
            drop(to_asr);
            let _ = thread.join();
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
        // **接続を落としたら、状態も繋がっていない側へ戻す。**
        //
        // 落としたのに Streaming のまま残ると、次の音声は「繋がっている」
        // 経路へ入り、もう無い送り先へ送って失敗する。接続先を変えたときに
        // 実際に通る道である。Aborted は Connecting / Streaming / Closing
        // からだけ効き、それ以外では何も起きない。
        let _ = self.session.apply(SessionInput::Aborted);
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
/// **応答をまだ待っている間は閉じない。** 2026-08-25 の実機で、喋り終わってから
/// 当時の既定 15 秒でセッションを閉じてしまい、背後の
/// コールドスタート(30〜60 秒)が終わる前に諦めていた。閉じることすら
/// 完了せず `closing timed out without finished` になり、未応答の待ちが
/// 破棄されていた。待ちが残っている限り、この経路で閉じてはならない。
fn idle_close_is_due(
    state: SessionState,
    gate_is_speaking: bool,
    has_pending_response: bool,
    last_speech_endpoint_at: Option<Instant>,
    idle_close_sec: u32,
    now: Instant,
) -> bool {
    state == SessionState::Streaming
        && !gate_is_speaking
        && !has_pending_response
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

/// 発話開始の確認中に短い確率低下が挟まっても、最初の音を失わない保持時間。
///
/// 設定のプリロールだけでは、開始確認と無音確認をまたぐ発話頭を覆えない。
/// 実機では開始しきい値 0.9、開始確認 200ms、無音確認 800ms に対して
/// SpeechStarted が最初の有声判定から最大 981ms 遅れ、500ms の保持では
/// 約 481ms が欠けた。二つの確認窓と VAD 1 フレームを最低限保持する。
fn effective_preroll_ms(settings: &Settings) -> u32 {
    settings.preroll_ms.max(
        settings
            .vad_min_speech_ms
            .saturating_add(settings.vad_min_silence_ms)
            .saturating_add(VAD_FRAME_MS),
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
        spawn_connection_control, AsrTransport, ConnectionControlEvent, Controller, DelayedTurn,
        LevelStatus, OverlayKind, OverlayView, VadFrame, WarmupJob, WarmupReason, WarmupResult,
        ENROLL_RETRY_INITIAL, ENROLL_RETRY_MAX, FAILED_RETRY_INITIAL, FAILED_RETRY_MAX,
        GATEWAY_URL_MISSING_MESSAGE, OVERLAY_NOTICE_DURATION, PENDING_AUDIO_MAX_BYTES,
        SERVER_RESPONSE_STARTING_OVERLAY_DELAY, SERVER_RESPONSE_WAITING_OVERLAY_DELAY,
        WARMUP_IDLE_THRESHOLD,
    };
    use crate::connection::SelfHostedProvider;
    use crate::settings::Settings;
    use otoa_input_core::{
        Account, ConnectionControl, ConnectionProvider, Endpoint, EnrollOutcome, EnrollReason,
        GateEvent, PasteShortcutSetting, PrepareAction, Readiness, SessionInput, SessionState,
    };
    use otoa_input_protocol::{
        AsrCommand, AsrError, AsrEvent, AsrToken, POLICY_VIOLATION_CLOSE_CODE,
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    };
    use std::thread;
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

    #[test]
    fn superseded_connection_is_completed_only_after_audio_stop_is_acknowledged() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
        let address = listener.local_addr().expect("test address should be known");
        let (request_tx, request_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            for state in [Some("superseded"), None] {
                let (mut stream, _) = listener.accept().expect("test request should arrive");
                let mut bytes = [0_u8; 2_048];
                let count = stream
                    .read(&mut bytes)
                    .expect("test request should be read");
                let request = String::from_utf8_lossy(&bytes[..count]);
                let method = request.split_whitespace().next().unwrap_or("").to_string();
                request_tx
                    .send(method)
                    .expect("test request should be recorded");
                let body = state
                    .map(|state| format!(r#"{{"state":"{state}"}}"#))
                    .unwrap_or_default();
                let status = if state.is_some() {
                    "200 OK"
                } else {
                    "204 No Content"
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("test response should be written");
            }
        });
        let control = ConnectionControl {
            status_url: format!("http://{address}/status"),
            complete_url: format!("http://{address}/complete"),
            headers: Vec::new(),
            poll_interval_ms: 1,
        };

        let (stop, events, worker) = spawn_connection_control(control);

        assert_eq!(
            request_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "GET"
        );
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(1)),
            Ok(ConnectionControlEvent::Superseded)
        ));
        assert!(request_rx.recv_timeout(Duration::from_millis(150)).is_err());

        stop.send(())
            .expect("audio stop acknowledgement should be sent");
        assert_eq!(
            request_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "POST"
        );
        worker.join().expect("control worker should stop");
        server.join().expect("test server should stop");
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
                control: None,
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
                control: None,
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
                control: None,
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
        let (mut controller, _asr_commands) = streaming_controller(settings);
        assert!(
            !matches!(
                controller.session.state(),
                SessionState::Disabled | SessionState::Failed
            ),
            "待受中であること"
        );

        // 実際の接続スレッドと同じく、指示の受け口が閉じるまで生きる。
        // 何もしない試験用スレッドでは、設定変更側が join で固まっても
        // 再現できない。
        let (to_asr, commands) = crossbeam_channel::unbounded();
        let (_events_tx, events) = crossbeam_channel::unbounded();
        let connection_exited = Arc::new(AtomicBool::new(false));
        let exited_by_thread = connection_exited.clone();
        let thread = thread::spawn(move || {
            while commands.recv().is_ok() {}
            exited_by_thread.store(true, Ordering::SeqCst);
        });
        controller.asr = Some(AsrTransport {
            to_asr,
            events,
            thread,
            control_stop: None,
            control_events: None,
            control_thread: None,
        });

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
        assert!(controller.asr.is_none(), "張ってある接続を切ること");
        assert!(
            connection_exited.load(Ordering::SeqCst),
            "古い接続の終了待ちで設定変更が停止している"
        );
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
        while controller.warmup.is_some() && Instant::now() < deadline {
            controller.drain_warmup_events();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            controller.warmup.is_none(),
            "warmup worker did not report completion"
        );
    }

    #[test]
    fn the_warming_overlay_stays_until_the_warmup_finishes() {
        let (mut controller, calls) = warmup_controller(Settings::default());

        assert!(controller.start_warmup(WarmupReason::BeforeSpeech));
        assert!(controller.warmup.is_some());
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

    /// 「まだ話さないでください」と、貼り付けを止める内部状態は同時に終わる。
    ///
    /// 途中結果が届いたときだけ表示を認識中へ変えると、画面では準備が終わった
    /// ように見えるのに、確定時には貼り付けないという食い違いが生まれる。
    #[test]
    fn a_delayed_turn_keeps_the_warming_overlay_until_the_turn_finishes() {
        let (mut controller, _calls) = warmup_controller(Settings::default());
        controller.delayed_turn = Some(DelayedTurn {
            began_at: Instant::now(),
            speech_ended: false,
        });
        controller.transcript.replace_partial("途中結果");

        // 1 回目で途中結果を表示用の事実へ反映し、2 回目でも表示と内部状態が
        // ずれないことを確かめる。
        controller.refresh_overlay();
        controller.refresh_overlay();

        assert!(controller.delayed_turn.is_some());
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WarmingUp,
                ..
            }
        ));
    }

    /// サーバーが 1 回の発話を複数の `<end>` に分けても、途中から貼らない。
    ///
    /// 遅い回線では、暖機中に溜めた 1 発話が「本文」「短い語尾」の順で返ることが
    /// ある。最初の区切りで待たせた印を消すと、警告中に喋った同じ発話なのに
    /// 2 区切り目だけが貼られる。
    #[test]
    fn every_segment_of_a_delayed_turn_stays_delayed_until_the_session_finishes() {
        let mut settings = Settings::default();
        settings.auto_paste = false;
        settings.paste_per_endpoint = true;
        settings.endpoint_mode = "server".to_string();
        let (mut controller, _calls) = warmup_controller(settings);
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        controller.delayed_turn = Some(DelayedTurn {
            began_at: Instant::now(),
            speech_ended: true,
        });

        controller.handle_asr_event(AsrEvent::Connected);
        assert!(
            controller
                .delayed_turn
                .as_ref()
                .is_some_and(|turn| turn.speech_ended),
            "接続完了処理が、端末で確認済みの発話終了を取り消している"
        );

        controller.transcript.push_final("本文");
        controller.handle_asr_event(AsrEvent::Endpoint);

        assert_eq!(
            controller.session.state(),
            SessionState::Closing,
            "待たせた発話の接続を閉じず、次の発話との境界が曖昧なままになっている"
        );
        assert!(
            controller.delayed_turn.is_some(),
            "最初の区切りで待たせた印を消している"
        );
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WarmingUp,
                ..
            }
        ));

        controller.transcript.push_final("語尾");
        controller.handle_asr_event(AsrEvent::Endpoint);
        assert!(
            controller.delayed_turn.is_some(),
            "後続区切りが届く前に待たせた印を消している"
        );

        controller.cleanup_asr();
        assert!(controller.delayed_turn.is_none());
    }

    /// 文字が返らなかった発話でも、専用接続の終了と一緒に準備中を終える。
    #[test]
    fn an_empty_delayed_turn_finishes_its_warming_state_with_the_session() {
        let mut settings = Settings::default();
        settings.auto_paste = true;
        settings.paste_per_endpoint = true;
        let (mut controller, _calls) = warmup_controller(settings);
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        assert!(controller.session.apply(SessionInput::Connected));
        controller.delayed_turn = Some(DelayedTurn {
            began_at: Instant::now(),
            speech_ended: true,
        });

        controller.handle_asr_event(AsrEvent::Endpoint);

        assert_eq!(controller.session.state(), SessionState::Closing);
        assert!(
            controller.delayed_turn.is_some(),
            "接続の後続応答より先に準備中を終えている"
        );
        controller.cleanup_asr();

        assert!(
            controller.delayed_turn.is_none(),
            "接続が終わったのに準備中の内部状態が残っている"
        );
    }

    #[test]
    fn cleanup_closes_the_asr_command_channel_before_waiting_for_its_thread() {
        let mut controller = test_controller(Settings::default());
        let (to_asr, commands) = crossbeam_channel::unbounded();
        let (_events_tx, events) = crossbeam_channel::unbounded();
        let exited = Arc::new(AtomicBool::new(false));
        let exited_by_thread = exited.clone();
        let thread = thread::spawn(move || {
            while commands.recv().is_ok() {}
            exited_by_thread.store(true, Ordering::SeqCst);
        });
        controller.asr = Some(AsrTransport {
            to_asr,
            events,
            thread,
            control_stop: None,
            control_events: None,
            control_thread: None,
        });

        controller.cleanup_asr();

        assert!(
            exited.load(Ordering::SeqCst),
            "終了待ちより前に指示の送り口が閉じられていない"
        );
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

        assert!(controller.warmup.is_some());
        assert!(controller.deferred_speech.is_some());
        assert_eq!(controller.session.state(), SessionState::Listening);
        assert!(controller.asr.is_none());
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
        assert!(controller.warmup.is_some(), "暖機が始まっていない");
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
        assert!(
            controller
                .delayed_turn
                .as_ref()
                .is_some_and(|turn| !turn.speech_ended),
            "認識接続の準備が終わる前に、暖機から始めた発話を見失っている"
        );

        controller.handle_asr_event(AsrEvent::Connected);

        assert!(
            controller.delayed_turn.is_none(),
            "発話中に認識接続まで準備できたのに、貼り付け禁止の印を残している"
        );
    }

    /// 発話が終わった後まで暖機を待たせた場合は、安全のため自動で貼らない。
    #[test]
    fn a_warmup_that_outlasts_speech_keeps_that_turn_delayed() {
        let (mut controller, _calls) = warmup_controller(Settings::default());
        controller.last_confident_speech_at = Some(Instant::now());
        assert!(controller.session.apply(SessionInput::Enable));
        controller.last_successful_asr_response_at = Some(Instant::now() - WARMUP_IDLE_THRESHOLD);

        controller.handle_gate_event(GateEvent::SpeechStarted);
        assert!(controller.warmup.is_some(), "暖機が始まっていない");
        controller.handle_gate_event(GateEvent::SpeechEnded);
        assert!(
            controller
                .deferred_speech
                .as_ref()
                .is_some_and(|speech| speech.ended),
            "暖機中に発話が終わったことを覚えていない"
        );

        wait_for_warmup(&mut controller);

        assert!(
            controller
                .delayed_turn
                .as_ref()
                .is_some_and(|turn| turn.speech_ended),
            "発話終了後まで待たせたのに、貼り付け禁止の印が無い"
        );
    }

    /// **待たせた印を、次の発話へ漏らさない。**
    ///
    /// 印は「その発話を貼らずにクリップボードへ置く」ためのもので、結果が
    /// 出ないまま終わったら一緒に消えなければならない。消し損ねると、次の
    /// 無関係な発話が貼られなくなる。旗を 3 つに分けて持っていたときは、
    /// 通知・空結果・接続失敗・停止のどれでも消えなかった。
    #[test]
    fn a_delayed_turn_does_not_leak_into_the_next_speech() {
        let (mut controller, _calls) = warmup_controller(Settings::default());
        assert!(controller.session.apply(SessionInput::Enable));
        controller.delayed_turn = Some(DelayedTurn {
            began_at: Instant::now(),
            speech_ended: false,
        });

        controller.show_overlay_notice("gate_blocked".to_string(), "一致しません".to_string());
        assert!(
            controller.delayed_turn.is_none(),
            "通知で終わったのに印が残っている"
        );

        controller.delayed_turn = Some(DelayedTurn {
            began_at: Instant::now(),
            speech_ended: false,
        });
        controller.cleanup_asr();
        assert!(
            controller.delayed_turn.is_none(),
            "接続を落としたのに印が残っている"
        );
    }

    /// **直前の登録が通っているなら、接続前にもう一度往復しない。**
    ///
    /// ここはコントローラのスレッドを止めて往復する。遠隔が遅い日は、そのぶん
    /// 接続が遅れて 1 発話目が間に合わなくなる。
    #[test]
    fn a_fresh_enrollment_is_not_sent_again_before_connecting() {
        let (mut controller, calls) = warmup_controller(Settings::default());
        controller.last_successful_asr_response_at = Some(Instant::now());

        assert!(controller.ensure_enrolled_before_connection());
        assert_eq!(calls.load(Ordering::SeqCst), 0, "同じ登録を送り直している");
    }

    /// **登録が古ければ送る。** 向こうの登録はインスタンスの記憶にしかないので、
    /// 時間が経てば消えている。
    #[test]
    fn a_stale_enrollment_is_sent_before_connecting() {
        let (mut controller, calls) = warmup_controller(Settings::default());
        controller.last_successful_asr_response_at = Some(Instant::now() - WARMUP_IDLE_THRESHOLD);

        assert!(controller.ensure_enrolled_before_connection());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "古い登録を送り直していない"
        );
    }

    /// **途中結果が届いたら待ちを解く。**
    ///
    /// 解かないと、応答が届いているのに古い起点が居座り、10 秒を超えて
    /// 「サーバーを起動しています」に変わる。喋り続けているのに出る、という
    /// 形で実際に起きた。
    #[test]
    fn a_partial_result_ends_the_wait_for_the_server() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr) = streaming_controller(settings);
        end_speech(&mut controller);
        age_oldest_pending_wait(&mut controller, SERVER_RESPONSE_STARTING_OVERLAY_DELAY);

        controller.handle_asr_event(AsrEvent::PartialText(vec![AsrToken {
            text: "とちゅう".to_string(),
            start_ms: None,
            end_ms: None,
            confidence: None,
            is_final: false,
            speaker: None,
            language: None,
            translation_status: None,
            source_language: None,
        }]));

        assert!(
            controller.facts.pending_since.is_none(),
            "途中結果が届いたのに待ちが残っている"
        );
        controller.check_server_response_wait_overlay();
        assert!(
            !matches!(
                controller.overlay,
                OverlayView::Shown {
                    kind: OverlayKind::StartingServer,
                    ..
                }
            ),
            "応答が届いているのにサーバー起動中を出している"
        );
    }

    /// **サーバーが先に終話を告げたら、待ちを作り直さない。**
    ///
    /// 端末の VAD より先に `<end>` が届くことがある。そこで新しい待ちを作ると、
    /// 対応する応答はもう来ないので永久に残る。
    #[test]
    fn a_server_endpoint_before_the_gate_does_not_open_a_new_wait() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr) = streaming_controller(settings);

        controller.handle_asr_event(AsrEvent::Endpoint);
        end_speech(&mut controller);

        assert!(
            controller.facts.pending_since.is_none(),
            "サーバーが答えた後に待ちを作っている"
        );
    }

    /// **待受に入っただけでは、向こうを起こさない。**
    ///
    /// 認識器はゼロまで落ちる配備なので、起こせばその間ずっと課金される。
    /// 喋っていない人のために起こし続ける理由は無い。登録は最初の発話の
    /// ときにまとめてやり、待たせているあいだは「準備中」を出す。
    #[test]
    fn listening_does_not_wake_the_recognizer_on_its_own() {
        let (mut controller, calls) = warmup_controller(Settings::default());

        controller.enable_listening();

        assert!(controller.warmup.is_none(), "喋る前に起こしている");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "喋る前に登録を送っている");
    }

    /// 最初の発話で、そのとき初めて登録する。
    #[test]
    fn the_first_speech_enrolls_and_is_held_until_it_finishes() {
        let (mut controller, calls) = warmup_controller(Settings::default());
        assert!(controller.session.apply(SessionInput::Enable));

        controller.handle_gate_event(GateEvent::SpeechStarted);

        assert!(controller.warmup.is_some(), "喋ったのに登録していない");
        assert!(
            controller.deferred_speech.is_some(),
            "登録のあいだ発話を預かっていない"
        );
        wait_for_warmup(&mut controller);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// **暖機が終わる前に、保留した発話で接続を始めない。**
    ///
    /// 遅い日に時間だけで見切ると、まだ走っている登録と ASR 接続が競合して
    /// 接続が切れる。画面が「まだ話さないでください」のあいだは内部も同じ
    /// 状態に留まり、登録自身の成功・失敗を唯一の終了条件にする。
    #[test]
    fn speech_stays_held_while_the_warmup_is_running() {
        let (mut controller, _calls) = warmup_controller(Settings::default());
        assert!(controller.session.apply(SessionInput::Enable));
        controller.warmup = Some(WarmupJob::for_test(
            Instant::now() - Duration::from_secs(60),
        ));

        controller.handle_gate_event(GateEvent::SpeechStarted);

        assert!(
            controller.deferred_speech.is_some(),
            "暖機中なのに保留を解いて接続を始めている"
        );
        assert_eq!(controller.session.state(), SessionState::Listening);
    }

    /// 周期処理も、暖機が返る前に時間だけで保留を解かない。
    #[test]
    fn the_clock_does_not_release_speech_before_the_warmup_finishes() {
        let (mut controller, _calls) = warmup_controller(Settings::default());
        assert!(controller.session.apply(SessionInput::Enable));
        controller.warmup = Some(WarmupJob::for_test(
            Instant::now() - Duration::from_secs(60),
        ));

        controller.handle_gate_event(GateEvent::SpeechStarted);
        assert!(controller.deferred_speech.is_some(), "発話を預かっていない");

        controller.drain_warmup_events();
        assert!(
            controller.deferred_speech.is_some(),
            "暖機の結果が無いのに時計だけで保留を解いている"
        );
    }

    /// **保留を上書きしない。** 上書きすると、暖機が長引いたときに
    /// 最後の 1 発話しか残らない。
    #[test]
    fn a_second_speech_during_the_warmup_does_not_replace_the_first() {
        let (mut controller, _calls) = warmup_controller(Settings::default());
        assert!(controller.session.apply(SessionInput::Enable));
        controller.warmup = Some(WarmupJob::for_test(Instant::now()));
        controller.preroll.push(&[2_i16; 160]);

        controller.handle_gate_event(GateEvent::SpeechStarted);
        controller.handle_vad_samples(&[1_i16; 160]);
        controller.handle_gate_event(GateEvent::SpeechEnded);
        let after_first = controller
            .deferred_speech
            .as_ref()
            .expect("1 回目の保留")
            .audio
            .len();

        controller.handle_gate_event(GateEvent::SpeechStarted);
        controller.handle_vad_samples(&[1_i16; 160]);
        let after_second = controller
            .deferred_speech
            .as_ref()
            .expect("2 回目でも保留が残ること")
            .audio
            .len();

        assert!(
            after_second > after_first,
            "2 回目が 1 回目を捨てている: {after_first} -> {after_second}"
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

        assert!(controller.warmup.is_some());
        assert!(matches!(
            controller.warmup.as_ref().map(|job| job.reason),
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
    fn temporary_warmup_failure_returns_to_a_retryable_state() {
        let (mut controller, _calls) = warmup_controller(Settings::default());
        // 暖機は「最近使った」ときだけ続く。
        controller.last_confident_speech_at = Some(Instant::now());
        controller.warmup = Some(WarmupJob::for_test(Instant::now()));
        controller.set_overlay(OverlayView::Shown {
            kind: OverlayKind::WarmingUp,
            committed: String::new(),
            partial: String::new(),
            error: String::new(),
        });

        controller.finish_warmup(WarmupResult {
            reason: WarmupReason::BeforeSpeech,
            started_at: Instant::now(),
            result: Err(anyhow::anyhow!("gateway timed out while starting")),
        });

        assert!(controller.warmup.is_none());
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

        controller.handle_gate_event(GateEvent::SpeechStarted);
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
        // **登録が通らなくても、預かった発話は送る。** ゲートウェイは保存して
        // ある参照音声から復帰できるので、認識は始められる。
        assert_ne!(controller.session.state(), SessionState::Listening);

        std::thread::sleep(Duration::from_millis(110));
        controller.handle_gate_event(GateEvent::SpeechStarted);
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
        controller.asr = Some(AsrTransport::for_test(to_asr));
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        assert!(controller.session.apply(SessionInput::Connected));
        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);
        assert!(matches!(
            asr_commands.try_recv(),
            Ok(AsrCommand::SpeechStart)
        ));
        (controller, asr_commands)
    }

    fn end_speech(controller: &mut Controller) {
        assert_eq!(controller.gate.push(0.0), Some(GateEvent::SpeechEnded));
        controller.handle_gate_event(GateEvent::SpeechEnded);
    }

    fn age_oldest_pending_wait(controller: &mut Controller, elapsed: Duration) {
        assert!(
            controller.facts.pending_since.is_some(),
            "無音の検知で応答待ちが始まるはず"
        );
        controller.facts.pending_since = Some(Instant::now() - elapsed);
        controller.sync_pending_wait_observer();
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

    /// **1 発話ぶんは必ず抱えられること。** フレーム数で切っていたときは
    /// 約 3.2 秒しか入らず、接続が少しでも遅れると発話の頭から溢れて、
    /// 古い順に黙って捨てていた。
    #[test]
    fn pending_audio_holds_a_whole_utterance_while_the_connection_opens() {
        let mut controller = test_controller(Settings::default());
        // 16kHz s16 の 1 フレーム（32ms 相当）を 20 秒ぶん積む。
        let frame = vec![0_u8; 512 * 2];
        let frames_for_20s = (16_000 * 2 * 20) / frame.len();
        for _ in 0..frames_for_20s {
            controller.queue_pending_audio(frame.clone());
        }

        assert_eq!(
            controller.pending_audio_dropped_frames, 0,
            "20 秒ぶんで捨てている"
        );
        assert_eq!(controller.pending_audio.len(), frames_for_20s);
    }

    #[test]
    fn preroll_covers_gate_confirmation_and_one_silence_gap() {
        let settings = settings_with(|settings| {
            settings.preroll_ms = 500;
            settings.vad_min_speech_ms = 200;
            settings.vad_min_silence_ms = 800;
        });
        let mut controller = test_controller(settings);
        controller
            .preroll
            .push(&vec![1; super::milliseconds_to_samples(1_100)]);

        assert_eq!(
            controller.preroll.take().len(),
            super::milliseconds_to_samples(1_032),
            "開始判定中の短い確率低下をまたいだ発話頭まで残す"
        );
    }

    /// 上限を超えたら、古い方から捨てて新しい方を残す。
    #[test]
    fn pending_audio_keeps_the_newest_audio_and_records_the_drop() {
        let mut controller = test_controller(Settings::default());
        let frame = vec![0_u8; PENDING_AUDIO_MAX_BYTES / 4];
        for _ in 0..4 {
            controller.queue_pending_audio(frame.clone());
        }
        assert_eq!(controller.pending_audio_dropped_frames, 0);

        controller.queue_pending_audio(vec![7_u8; frame.len()]);
        assert_eq!(controller.pending_audio_dropped_frames, 1);
        assert_eq!(controller.pending_audio.len(), 4);
        assert_eq!(controller.pending_audio.last().map(|f| f[0]), Some(7));
    }

    #[test]
    fn a_twenty_five_second_gateway_handshake_keeps_the_first_speech_buffered() {
        let mut controller = test_controller(Settings::default());
        let (to_asr, _commands) = crossbeam_channel::unbounded();
        controller.asr = Some(AsrTransport::for_test(to_asr));
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        controller.connecting_started_at = Some(Instant::now() - Duration::from_secs(25));
        controller.pending_audio.push(vec![1, 2, 3, 4]);

        controller.check_session_timeouts();

        assert_eq!(controller.session.state(), SessionState::Connecting);
        assert_eq!(controller.pending_audio, vec![vec![1, 2, 3, 4]]);
    }

    #[test]
    fn speech_that_ends_during_handshake_is_queued_as_one_complete_turn() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "both".to_string();
        });
        let mut controller = test_controller(settings);
        let (to_asr, asr_commands) = crossbeam_channel::unbounded();
        controller.asr = Some(AsrTransport::for_test(to_asr));
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        controller.pending_audio.push(vec![1, 2, 3, 4]);

        controller.finish_current_speech();

        assert!(matches!(
            asr_commands.try_recv(),
            Ok(AsrCommand::Audio(bytes)) if bytes == vec![1, 2, 3, 4]
        ));
        assert!(matches!(asr_commands.try_recv(), Ok(AsrCommand::Finalize)));
        assert!(controller.client_finalize_sent);
        assert!(controller.finalize_pending);

        controller.handle_asr_event(AsrEvent::Connected);
        assert_eq!(controller.session.state(), SessionState::Streaming);
        assert!(asr_commands.try_recv().is_err());
    }

    #[test]
    fn two_speeches_during_handshake_keep_their_server_turn_boundary() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "both".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let mut controller = test_controller(settings);
        let (to_asr, asr_commands) = crossbeam_channel::unbounded();
        controller.asr = Some(AsrTransport::for_test(to_asr));
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        controller.pending_audio.push(vec![1, 2]);
        controller.finish_current_speech();
        controller.handle_vad_samples(&[3]);

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);
        controller.handle_vad_samples(&[4]);
        controller.handle_asr_event(AsrEvent::Connected);

        assert!(matches!(
            asr_commands.try_recv(),
            Ok(AsrCommand::Audio(bytes)) if bytes == vec![1, 2]
        ));
        assert!(matches!(asr_commands.try_recv(), Ok(AsrCommand::Finalize)));
        assert!(matches!(
            asr_commands.try_recv(),
            Ok(AsrCommand::SpeechStart)
        ));
        assert!(matches!(
            asr_commands.try_recv(),
            Ok(AsrCommand::Audio(bytes)) if bytes == 3_i16.to_le_bytes()
        ));
        assert!(matches!(
            asr_commands.try_recv(),
            Ok(AsrCommand::Audio(bytes)) if bytes == 4_i16.to_le_bytes()
        ));
        assert!(asr_commands.try_recv().is_err());
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
    fn identical_text_from_two_turns_is_committed_twice() {
        let settings = settings_with(|settings| {
            settings.auto_paste = false;
            settings.commit_hold_ms = 800;
        });
        let mut controller = test_controller(settings);

        controller.commit_segment(Some("同じ発話".to_string()));
        controller.facts.commit = None;
        controller.commit_segment(Some("同じ発話".to_string()));

        assert!(matches!(
            &controller.facts.commit,
            Some((text, _)) if text == "同じ発話"
        ));
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
        let (to_asr, asr_commands) = crossbeam_channel::unbounded();
        controller.asr = Some(AsrTransport::for_test(to_asr));
        controller.commit_segment(Some("前の発話".to_string()));
        assert!(controller.committed_hold_until.is_some());

        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        assert!(controller.session.apply(SessionInput::Connected));
        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);

        assert!(matches!(
            asr_commands.try_recv(),
            Ok(AsrCommand::SpeechStart)
        ));

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
        controller.asr = Some(AsrTransport::for_test(to_asr));
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
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        age_oldest_pending_wait(&mut controller, Duration::from_millis(2_580));
        controller.check_server_response_wait_overlay();

        assert_eq!(controller.overlay, OverlayView::Hidden);
    }

    #[test]
    fn pending_wait_changes_at_four_and_ten_seconds() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        age_oldest_pending_wait(&mut controller, SERVER_RESPONSE_WAITING_OVERLAY_DELAY);
        controller.check_server_response_wait_overlay();
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WaitingForResponse,
                ..
            }
        ));

        age_oldest_pending_wait(&mut controller, SERVER_RESPONSE_STARTING_OVERLAY_DELAY);
        controller.check_server_response_wait_overlay();
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::StartingServer,
                ..
            }
        ));
    }

    #[test]
    fn pending_wait_is_not_rewound_when_the_next_speech_starts() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        age_oldest_pending_wait(&mut controller, SERVER_RESPONSE_WAITING_OVERLAY_DELAY);
        let first_started_at = controller.facts.pending_since.expect("first wait");
        controller.check_server_response_wait_overlay();

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);
        assert_eq!(controller.facts.pending_since, Some(first_started_at));
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WaitingForResponse,
                ..
            }
        ));

        end_speech(&mut controller);
        // 積み増さない。開始時刻は「待っていない状態から最初に送った時刻」を保つ。
        assert_eq!(controller.facts.pending_since, Some(first_started_at));
    }

    #[test]
    fn old_response_cannot_pop_the_newer_speech_wait_or_hide_its_overlay() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.auto_paste = false;
            settings.commit_hold_ms = 0;
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        let first_since = controller.facts.pending_since.expect("first wait");

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);
        end_speech(&mut controller);
        let second_since = controller.facts.pending_since.expect("second wait");

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);
        controller.handle_asr_event(AsrEvent::FinalText(vec![asr_token("古い結果", true)]));
        controller.handle_asr_event(AsrEvent::Endpoint);

        // 2026-08-25: 待ちは常に最大 1 件である。端末の VAD が無音を検知しても
        // サーバーは終話と判断したときだけ返すので、複数回の検知が 1 つの応答に
        // まとめられる。1 対 1 に数えると行列が伸び続け、先頭が古いまま残って
        // 待ち表示が消えなくなる(実機で発生)。したがって 2 回目以降は積み増さず、
        // 応答が来れば空にする。
        assert_eq!(first_since, second_since, "次の発話で待ちを作り直している");
        assert!(controller.facts.pending_since.is_none());
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::Recognizing,
                ..
            }
        ));
    }

    #[test]
    fn server_wait_shows_recognizing_after_one_and_a_half_seconds() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        controller.server_response_wait_started_at =
            Some(Instant::now() - SERVER_RESPONSE_WAITING_OVERLAY_DELAY);
        controller.check_server_response_wait_overlay();

        assert_eq!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WaitingForResponse,
                committed: String::new(),
                partial: String::new(),
                error: String::new(),
            }
        );
        assert!(controller.server_response_wait_overlay_visible);
    }

    #[test]
    fn server_wait_changes_to_starting_server_after_six_and_a_half_seconds() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        controller.server_response_wait_started_at =
            Some(Instant::now() - SERVER_RESPONSE_WAITING_OVERLAY_DELAY);
        controller.check_server_response_wait_overlay();
        controller.server_response_wait_started_at =
            Some(Instant::now() - SERVER_RESPONSE_STARTING_OVERLAY_DELAY);
        controller.check_server_response_wait_overlay();

        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::StartingServer,
                ..
            }
        ));
        assert!(controller.server_response_wait_overlay_visible);
    }

    #[test]
    fn server_wait_stays_visible_and_keeps_its_start_time_during_the_next_speech() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        let started_at = Instant::now() - SERVER_RESPONSE_WAITING_OVERLAY_DELAY;
        controller.server_response_wait_started_at = Some(started_at);
        controller.check_server_response_wait_overlay();

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);

        assert_eq!(controller.server_response_wait_started_at, Some(started_at));
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
        assert_eq!(controller.server_response_wait_started_at, Some(started_at));
    }

    #[test]
    fn server_starting_wait_never_reverts_to_recognizing_for_a_new_speech() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        let started_at = Instant::now() - SERVER_RESPONSE_STARTING_OVERLAY_DELAY;
        controller.server_response_wait_started_at = Some(started_at);
        controller.check_server_response_wait_overlay();

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);
        end_speech(&mut controller);
        controller.check_server_response_wait_overlay();

        assert_eq!(controller.server_response_wait_started_at, Some(started_at));
        assert!(matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::StartingServer,
                ..
            }
        ));
    }

    #[test]
    fn confirmed_server_transcript_clears_the_waiting_overlay() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        controller.server_response_wait_started_at =
            Some(Instant::now() - SERVER_RESPONSE_WAITING_OVERLAY_DELAY);
        controller.check_server_response_wait_overlay();
        controller.handle_asr_event(AsrEvent::FinalText(vec![asr_token("確定結果", true)]));

        assert!(controller.server_response_wait_started_at.is_none());
        assert!(!controller.server_response_wait_overlay_visible);
        assert!(!matches!(
            controller.overlay,
            OverlayView::Shown {
                kind: OverlayKind::WaitingForResponse | OverlayKind::StartingServer,
                ..
            }
        ));
    }

    #[test]
    fn server_response_before_wait_delay_never_shows_a_waiting_overlay() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        controller.handle_asr_event(AsrEvent::Endpoint);

        assert!(controller.server_response_wait_started_at.is_none());
        assert!(!controller.server_response_wait_overlay_visible);
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
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        controller.server_response_wait_started_at =
            Some(Instant::now() - SERVER_RESPONSE_WAITING_OVERLAY_DELAY);
        controller.check_server_response_wait_overlay();

        controller.handle_asr_event(AsrEvent::FinalText(vec![asr_token("確定結果", true)]));
        controller.handle_asr_event(AsrEvent::Endpoint);

        assert!(controller.server_response_wait_started_at.is_none());
        assert!(!controller.server_response_wait_overlay_visible);
        assert_eq!(controller.overlay, OverlayView::Hidden);
        assert!(controller.transcript.is_empty());
        assert!(controller.pending_commit.is_empty());
    }

    #[test]
    fn fast_server_response_never_shows_a_waiting_overlay() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        controller.server_response_wait_started_at = Some(
            Instant::now() - (SERVER_RESPONSE_WAITING_OVERLAY_DELAY - Duration::from_millis(100)),
        );
        controller.check_server_response_wait_overlay();

        assert_eq!(controller.overlay, OverlayView::Hidden);
        assert!(!controller.server_response_wait_overlay_visible);
    }

    #[test]
    fn server_response_notice_replaces_the_waiting_overlay() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        controller.server_response_wait_started_at =
            Some(Instant::now() - SERVER_RESPONSE_WAITING_OVERLAY_DELAY);
        controller.check_server_response_wait_overlay();
        controller.handle_asr_event(AsrEvent::Notice {
            code: "gate_blocked".to_string(),
            message: "登録した声と一致しませんでした。".to_string(),
        });

        assert!(controller.server_response_wait_started_at.is_none());
        assert!(!controller.server_response_wait_overlay_visible);
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
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        controller.server_response_wait_started_at =
            Some(Instant::now() - SERVER_RESPONSE_WAITING_OVERLAY_DELAY);
        controller.check_server_response_wait_overlay();
        controller.handle_asr_event(AsrEvent::Failed(AsrError::Server {
            code: 503,
            error_type: "unavailable".to_string(),
            message: "starting".to_string(),
            request_id: None,
        }));

        assert!(controller.server_response_wait_started_at.is_none());
        assert!(!controller.server_response_wait_overlay_visible);
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
        assert!(matches!(
            asr_commands.try_recv(),
            Ok(AsrCommand::SpeechStart)
        ));
        match asr_commands.try_recv() {
            Ok(AsrCommand::Audio(bytes)) => {
                assert_eq!(bytes, vec![101, 0, 54, 255]);
            }
            _ => panic!("next speech should resume audio with the saved preroll"),
        }
        assert!(asr_commands.try_recv().is_err());
    }

    #[test]
    fn late_endpoint_keeps_the_next_speech_preroll_before_start_confirmation() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "both".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);
        assert!(matches!(asr_commands.try_recv(), Ok(AsrCommand::Finalize)));

        // The first turn's result can arrive after the microphone has already
        // started collecting the next turn, but before SpeechGate has emitted
        // SpeechStarted. Those samples are the head of the next utterance.
        controller.handle_vad_samples(&[101, -202]);
        controller.handle_asr_event(AsrEvent::Endpoint);

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);

        assert!(matches!(
            asr_commands.try_recv(),
            Ok(AsrCommand::SpeechStart)
        ));
        match asr_commands.try_recv() {
            Ok(AsrCommand::Audio(bytes)) => {
                assert_eq!(bytes, vec![101, 0, 54, 255]);
            }
            _ => panic!("late endpoint discarded the next utterance's head"),
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

    /// 暖機中に始めた発話の応答が遅れ、次の発話中に届いても接続を閉じない。
    /// 閉じると、その時点まで送った次発話の音声が応答前に捨てられる。
    #[test]
    fn delayed_endpoint_received_during_new_speech_does_not_close_the_session() {
        let settings = settings_with(|settings| {
            settings.auto_paste = false;
            settings.endpoint_mode = "server".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, _asr_commands) = streaming_controller(settings);
        controller.delayed_turn = Some(DelayedTurn {
            began_at: Instant::now(),
            speech_ended: true,
        });
        assert!(controller.gate.is_speaking());

        controller.handle_asr_event(AsrEvent::Endpoint);

        assert_eq!(controller.session.state(), SessionState::Streaming);
        assert!(controller.delayed_turn.is_some());

        end_speech(&mut controller);
        controller.handle_asr_event(AsrEvent::Endpoint);

        assert_eq!(controller.session.state(), SessionState::Closing);
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

        end_speech(&mut controller);
        assert!(controller.server_audio_paused, "VAD が黙ったら送信を止める");

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
            false,
            old_endpoint,
            15,
            now
        ));
        assert!(idle_close_is_due(
            SessionState::Streaming,
            false,
            false,
            old_endpoint,
            15,
            now
        ));
    }

    #[test]
    fn idle_close_never_fires_while_a_response_is_still_pending() {
        // 背後のコールドスタートは 30〜60 秒かかる。応答を待っている間に閉じると、
        // 起きる前に諦めることになり、その発話は永久に返らない。
        let now = Instant::now();
        let old_endpoint = Some(now - Duration::from_secs(16));

        assert!(!idle_close_is_due(
            SessionState::Streaming,
            false,
            true,
            old_endpoint,
            15,
            now
        ));
    }

    #[test]
    fn periodic_does_not_idle_close_while_finalize_response_is_pending() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "both".to_string();
            settings.idle_close_sec = 0;
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);
        end_speech(&mut controller);
        assert!(matches!(asr_commands.try_recv(), Ok(AsrCommand::Finalize)));
        controller.last_speech_endpoint_at = Some(Instant::now() - Duration::from_secs(1));

        controller.periodic();

        assert_eq!(controller.session.state(), SessionState::Streaming);
        assert!(controller.finalize_pending);
        assert!(asr_commands.try_recv().is_err());
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
        controller.asr = Some(AsrTransport::for_test(to_asr));
        assert!(controller.session.apply(SessionInput::Enable));
        assert!(controller.session.apply(SessionInput::SpeechStarted));
        assert!(controller.session.apply(SessionInput::Connected));
        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);

        assert_eq!(controller.gate.push(0.0), Some(GateEvent::SpeechEnded));
        controller.handle_gate_event(GateEvent::SpeechEnded);

        assert!(matches!(
            asr_commands.try_recv(),
            Ok(AsrCommand::SpeechStart)
        ));
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
    fn both_mode_finalizes_the_unclosed_server_segment_at_local_speech_end() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "both".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        end_speech(&mut controller);

        assert!(matches!(asr_commands.try_recv(), Ok(AsrCommand::Finalize)));
        assert!(controller.finalize_pending);
        assert!(controller.server_audio_paused);

        controller.handle_vad_samples(&[101, -202]);
        assert!(asr_commands.try_recv().is_err());

        assert_eq!(controller.gate.push(1.0), Some(GateEvent::SpeechStarted));
        controller.handle_gate_event(GateEvent::SpeechStarted);
        assert!(!controller.server_audio_paused);
        assert!(matches!(
            asr_commands.try_recv(),
            Ok(AsrCommand::SpeechStart)
        ));
        assert!(matches!(asr_commands.try_recv(), Ok(AsrCommand::Audio(_))));
    }

    #[test]
    fn speech_ending_frame_is_sent_before_finalize() {
        let settings = settings_with(|settings| {
            settings.endpoint_mode = "both".to_string();
            settings.vad_min_speech_ms = 0;
            settings.vad_min_silence_ms = 0;
        });
        let (mut controller, asr_commands) = streaming_controller(settings);

        controller.handle_vad_frame(VadFrame {
            probs: vec![0.0],
            samples: vec![11, 22],
        });

        assert!(matches!(
            asr_commands.try_recv(),
            Ok(AsrCommand::Audio(bytes))
                if bytes == [11_i16.to_le_bytes(), 22_i16.to_le_bytes()].concat()
        ));
        assert!(matches!(asr_commands.try_recv(), Ok(AsrCommand::Finalize)));
        assert!(asr_commands.try_recv().is_err());
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
        assert!(controller.pending_commit.is_empty());

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
    use super::WarmupReason;

    /// 切り替えて保存した直後に暖機を打つ理由が、ログから追えること。
    #[test]
    fn the_reason_has_its_own_name() {
        assert_eq!(WarmupReason::SettingsChanged.as_str(), "settings_changed");
        // 既存の理由と混ざらない。混ざると「なぜ暖めたか」が読めなくなる。
        assert_ne!(
            WarmupReason::SettingsChanged.as_str(),
            WarmupReason::BeforeSpeech.as_str()
        );
        assert_ne!(
            WarmupReason::SettingsChanged.as_str(),
            WarmupReason::VoiceChanged.as_str()
        );
    }
}
