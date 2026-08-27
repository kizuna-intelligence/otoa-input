//! Otoa Input のクライアント本体。
//!
//! 接続先の解決とログインは [`ConnectionProvider`] に委ねてある。
//! 自分のサーバーへ直接繋ぐ実装（[`SelfHostedProvider`]）を同梱しているので、
//! 何も渡さなくても単体で動く。別の接続先を使いたい場合は、
//! [`ConnectionProvider`] を実装して [`run`] に渡す。
//!
//! ```no_run
//! use std::sync::Arc;
//! # fn main() -> anyhow::Result<()> {
//! let provider = Arc::new(otoa_input_app::SelfHostedProvider);
//! otoa_input_app::run(otoa_input_app::Deps::new(provider))
//! # }
//! ```

use crossbeam_channel::{unbounded as unbounded_channel, Sender};
use floem::window::WindowId;
use otoa_input_core::ConnectionProvider;
use otoa_input_platform::{PasteMethod, TextOutput};
use std::{collections::HashMap, sync::Arc};

mod bundled_server;
mod check_connection;
mod connection;
mod controller;
mod model_download;
mod settings;
mod settings_io;
pub mod ui;
mod wiring;

pub use connection::SelfHostedProvider;
pub use controller::{ControllerCommand, LevelStatus, LoginState};
pub use settings::Settings;

/// 設定画面を差し替える。
///
/// 受け取るものは公開版の設定画面と同じで、返すのは画面そのものである。
/// 公開版の [`ui::settings_view::view`] を自分で呼べるので、
/// **公開版の画面を土台にして自分の欄を足す**形が取れる。
/// 設定画面に足す、配布ごとの面。
///
/// 返すのは面の中身だけでよい。レール（左の項目）も枠も公開版が描くので、
/// 見た目は自動で揃う。`label` はレールに出す名前。
#[derive(Clone)]
pub struct ExtraSettingsPage {
    pub label: &'static str,
    pub build: Arc<
        dyn Fn(Settings, ui::UiState, Sender<ControllerCommand>) -> floem::AnyView + Send + Sync,
    >,
    /// 保存ボタンが押されたときに呼ばれる。この面が持つ設定を書き込む。
    ///
    /// **保存ボタンは 1 つにする。** 面が自分の保存ボタンを持つと、
    /// 公開版の保存が画面を開いた時点の設定から組み直すので、面が書いた
    /// ぶんが消える（実際にそうなった）。面は「何を足すか」だけを言い、
    /// 書き込むのは公開版の保存に一本化する。
    pub apply: Arc<dyn Fn(&mut Settings) + Send + Sync>,
}

/// 設定画面の面。[`SettingsExtension::page`] で名指しする。
///
/// **公開しているのは、配布側がこの名前を書くからである。** 型が
/// 非公開のままだと `SettingsExtension` を組み立てられない
/// （実際に配布側で組もうとして詰まった）。
pub use wiring::SettingsPage;

/// 設定画面の行に付ける名前。**配布側から名指しで落とすためにある。**
///
/// 公開版の面をそのまま使いたいが、配布によっては意味を持たない行がある。
/// たとえば接続先がサーバー側で決まる配布では「認識エンジン」を選ばせても
/// 効かない。**選べるのに効かない項目は、無いほうがよい。**
///
/// 値を消したり並べ替えたりしないこと。配布側がこの名前を書いている。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[non_exhaustive]
pub enum SettingsRow {
    /// 一般 — 起動時に待受を始める
    StartListening,
    /// 一般 — 確定後に自動で貼り付ける
    AutoPaste,
    /// 一般 — 言語
    Language,
    /// 一般 — 発話が無いときに接続を閉じるまで
    IdleClose,
    /// マイク — 使うマイク
    Microphone,
    /// マイク — 入力ゲイン
    InputGain,
    /// マイク — 入力レベル
    InputLevel,
    /// 認識 — 認識エンジン
    AsrEngine,
    /// 認識 — 発話の区切り（無音）
    VadMinSilence,
    /// 認識 — 発話とみなす最小の長さ
    VadMinSpeech,
    /// 認識 — 拾いやすさ（VAD しきい値）
    VadThreshold,
    /// 認識 — 話し始めをさかのぼる長さ
    Preroll,
}

/// 公開版の面に手を入れる、配布ごとの指定。
///
/// [`ExtraSettingsPage`] が面を 1 つ**足す**のに対し、こちらは公開版が既に
/// 持っている面を**受け継いで作り替える**。面が増えないので、
/// 「声」と「認識」のように同じ話が 2 か所に分かれることがない。
///
/// 落とすだけ・足すだけでもよい。両方指定してもよい。
#[derive(Clone)]
pub struct SettingsExtension {
    /// どの面に手を入れるか。
    pub page: wiring::SettingsPage,
    /// 公開版が描く行のうち、落とすもの。
    pub hidden_rows: &'static [SettingsRow],
    /// 枠の中に、行として足す中身。`None` なら足さない。
    ///
    /// 公開版の行と並ぶので、[`ui::settings_view::setting_row`] で作ること。
    pub build_rows: Option<
        Arc<dyn Fn(Settings, ui::UiState, Sender<ControllerCommand>) -> floem::AnyView + Send + Sync>,
    >,
    /// 枠の**外**、面の末尾に足す中身。`None` なら足さない。
    ///
    /// 見出しを持つ塊や、行に収まらない大きいものはこちら。
    /// **行として枠に入れると、枠からはみ出して窓に収まらない**
    /// （実際にそうなった）。
    pub build_sections: Option<
        Arc<dyn Fn(Settings, ui::UiState, Sender<ControllerCommand>) -> floem::AnyView + Send + Sync>,
    >,
    /// 保存ボタンが押されたときに呼ばれる。**保存ボタンは 1 つにする**
    /// 理由は [`ExtraSettingsPage::apply`] と同じ。
    pub apply: Option<Arc<dyn Fn(&mut Settings) + Send + Sync>>,
}

impl SettingsExtension {
    /// 行を落とすだけの指定。
    pub fn hiding(page: wiring::SettingsPage, hidden_rows: &'static [SettingsRow]) -> Self {
        Self { page, hidden_rows, build_rows: None, build_sections: None, apply: None }
    }
}

pub type SettingsView = Arc<
    dyn Fn(Settings, ui::UiState, Sender<ControllerCommand>, WindowId) -> floem::AnyView
        + Send
        + Sync,
>;
// 公開版の画面が受け取る初期ページは差し替え側に渡していない。
// 差し替えた画面が自分の面構成を持つ以上、公開版のページ指定は意味を持たない。

/// [`run`] に差し込むもの。
///
/// **拡張点を増やすときは、それが永久に維持する API になることを承知で増やす。**
/// 接続先ごとの設定は [`Settings::product`] を通して各実装が自分で解釈する。
pub struct Deps {
    /// 接続先の解決、認証、アカウント表示を担う。
    ///
    /// [`ConnectionProvider::prepare`] が `None` を返す実装では、
    /// ログイン関係の UI（トレイの項目、設定画面のアカウント欄）は出ない。
    pub provider: Arc<dyn ConnectionProvider>,
    /// 設定画面に足す面。`None` なら足さない。
    ///
    /// 画面ごと差し替える [`Deps::settings_view`] と違い、こちらは
    /// 公開版の画面をそのまま使ったまま、面を 1 つ増やす。
    /// ふつうはこちらで足りる。
    pub extra_settings_page: Option<ExtraSettingsPage>,
    /// 公開版の面を受け継いで作り替える指定。空なら公開版のまま。
    ///
    /// 面を 1 つ足す [`Deps::extra_settings_page`] と違い、こちらは既にある
    /// 面の中身を差し替える。**ふつうはこちらで足りる。**
    pub settings_extensions: Vec<SettingsExtension>,
    /// 設定画面。`None` なら公開版のものを使う。
    ///
    /// 設定画面に欄を 1 つ足すためだけに個別の差し込み口を並べると、
    /// 欄の種類ごとに口が増えていく。画面ごと渡せるようにして、
    /// 足すものは各実装が自分で決める。
    pub settings_view: Option<SettingsView>,
}

impl Deps {
    /// 接続先だけを差し替える。設定画面は公開版のものを使う。
    pub fn new(provider: Arc<dyn ConnectionProvider>) -> Self {
        Self {
            provider,
            settings_view: None,
            extra_settings_page: None,
            settings_extensions: Vec::new(),
        }
    }
}

/// 設定ファイルを読む。[`Deps`] を組み立てるために先に必要になる。
pub fn load_settings() -> anyhow::Result<Settings> {
    settings_io::load()
}

/// 設定を保存する。
///
/// 面を足す配布（[`Deps::extra_settings_page`]）が自分の保存を持つときに要る。
/// **[`ControllerCommand::UpdateSettings`] を送るだけでは残らない。**
/// あれは動いている本体へ知らせるだけで、ファイルには書かない。
/// 送るだけにして、再起動のたびに設定が消えることがあった。
pub fn save_settings(settings: &Settings) -> anyhow::Result<()> {
    settings_io::save(settings)
}

/// 同梱の [`SelfHostedProvider`] で起動する。
pub fn run_self_hosted() -> anyhow::Result<()> {
    // **設定を読む前に呼ぶ。** 他の配布と設定ファイルを共有しないため。
    otoa_input_platform::set_app_directory("otoa-input-oss");
    run(Deps::new(Arc::new(SelfHostedProvider)))
}

/// `--help` の本文。オプションを増やしたらここも足す。
pub const USAGE: &str = "\
otoa-input — 話した内容をカーソル位置へ貼り付ける音声入力

使い方:
  otoa-input [オプション]
  otoa-input --serve [サーバーオプション]

オプション:
  --serve             ASR サーバーだけを動かす（画面は出さない）
                      詳細は otoa-input --serve --help
  --check-connection  接続先へ繋いで結果を表示し、終了する
  --paste-test [文字列]
                      本文を CLIPBOARD/PRIMARY に置き、既定の Shift+Insert
                      で 1 回貼り付けて状態をログへ出して終了する。
                      文字を入れたい場所にカーソルを置いてから実行する
  --preview-overlay=<状態>
                      音声・接続なしで入力バーを表示する。
                      splash/warming-up/connecting/listening/finalizing/committed/error/login
  --preview-settings[=<面>]
                      音声・接続なしで設定画面を表示する。
                      general/mic/asr/advanced/account/about
  -h, --help          この使い方を表示する
  -V, --version       版を表示する

設定はトレイアイコンの「設定」から変更する。設定ファイルの場所は
Linux なら ~/.config/otoa-input-oss/settings.json。

接続先が空のときは、同梱の otoa-asr-server を既定の設定で起動した
アドレス (ws://127.0.0.1:8770/asr/v1) へ繋ぐ。
";

fn server_arguments(arguments: &[String]) -> Option<Vec<String>> {
    arguments
        .iter()
        .any(|argument| argument == "--serve")
        .then(|| {
            arguments
                .iter()
                .filter(|argument| argument.as_str() != "--serve")
                .cloned()
                .collect()
        })
}

fn run_server(arguments: &[String]) -> anyhow::Result<()> {
    if otoa_asr_server::Config::help_requested(arguments) {
        print!(
            "{}",
            otoa_asr_server::USAGE.replace("otoa-asr-server", "otoa-input --serve")
        );
        return Ok(());
    }

    let environment = std::env::vars().collect::<HashMap<_, _>>();
    let config = otoa_asr_server::Config::from_sources(arguments, &environment)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(otoa_asr_server::run(config))
}

/// 本文を置いて既定の貼り付けを 1 回試し、状態をログへ出す。
///
/// 貼り付けは OS ごとに実装が違い、外部コマンド（Linux の xdotool / wtype）や
/// 権限（macOS のアクセシビリティ）に依存する。認識まで動いても貼り付けだけ
/// 失敗することがあるので、そこだけを切り離して試せるようにしてある。
fn paste_test(text: &str) -> i32 {
    let mut output = match TextOutput::new() {
        Ok(output) => output,
        Err(error) => {
            println!("NG: 貼り付けの初期化に失敗しました: {error:#}");
            return 2;
        }
    };
    match output.emit(text, PasteMethod::ClipboardAndPaste) {
        Ok(()) => {
            println!(
                "OK: 貼り付けを実行しました（{} 文字）",
                text.chars().count()
            );
            println!("    カーソル位置に文字が入っていれば成功です。");
            println!("    入っていない場合、クリップボードには入っているので貼り付け操作だけが失敗しています。");
            0
        }
        Err(error) => {
            println!("NG: 貼り付けに失敗しました: {error:#}");
            0
        }
    }
}

/// クライアントを起動する。
///
/// `--serve` が渡されていれば ASR サーバーだけを、`--check-connection` が
/// 渡されていれば接続確認だけを行う。
pub fn run(deps: Deps) -> anyhow::Result<()> {
    otoa_input_platform::console::use_utf8_output();
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    // サーバーモードは設定ファイル、多重起動ロック、音声デバイスの初期化より
    // 前に分岐する。`--serve` 自体はサーバーの設定オプションではないので、
    // Config へ渡す前に取り除く。
    if let Some(arguments) = server_arguments(&arguments) {
        return run_server(&arguments);
    }
    // 使い方の表示は、多重起動の判定より先に行う。あとに置くと、
    // 起動中の `--help` が「already running」で終わる。
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        print!("{USAGE}");
        return Ok(());
    }
    // 配布物の名前に版を入れていないので、**手元のものがどれかはここで見る。**
    // `--help` と同じく、多重起動の判定より先に置く。
    if arguments
        .iter()
        .any(|argument| argument == "--version" || argument == "-V")
    {
        println!("otoa-input {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if let Some(argument) = arguments
        .iter()
        .find(|argument| argument.starts_with("--preview-overlay"))
    {
        let value = argument
            .strip_prefix("--preview-overlay=")
            .ok_or_else(|| anyhow::anyhow!("--preview-overlay は =状態 で指定してください"))?;
        let scenario = wiring::PreviewScenario::parse(value).ok_or_else(|| {
            anyhow::anyhow!(
                "未知の preview 状態です: {value}（splash/warming-up/connecting/listening/finalizing/committed/error/login）"
            )
        })?;
        let settings = load_settings()?;
        let (to_ui, ui_updates) = unbounded_channel();
        let runtime = wiring::start_preview(settings.clone(), scenario, to_ui)?;
        return ui::run(
            settings,
            ui_updates,
            runtime,
            deps.settings_view,
            deps.extra_settings_page,
            deps.settings_extensions,
        );
    }

    if let Some(argument) = arguments
        .iter()
        .find(|argument| argument.starts_with("--preview-settings"))
    {
        let page_name = argument
            .strip_prefix("--preview-settings=")
            .unwrap_or("general");
        let page = wiring::SettingsPage::parse(page_name).ok_or_else(|| {
            anyhow::anyhow!(
                "未知の設定プレビュー面です: {page_name}（general/mic/asr/advanced/account/about）"
            )
        })?;
        let settings = load_settings()?;
        let (to_ui, ui_updates) = unbounded_channel();
        let runtime = wiring::start_preview(
            settings.clone(),
            wiring::PreviewScenario::Settings(page),
            to_ui,
        )?;
        return ui::run(
            settings,
            ui_updates,
            runtime,
            deps.settings_view,
            deps.extra_settings_page,
            deps.settings_extensions,
        );
    }

    if let Some(index) = arguments
        .iter()
        .position(|argument| argument == "--paste-test")
    {
        let text = arguments
            .get(index + 1)
            .filter(|argument| !argument.starts_with("--"))
            .cloned()
            .unwrap_or_else(|| "Otoa Input 貼り付けテスト".to_string());
        std::process::exit(paste_test(&text));
    }

    let check_connection = arguments
        .iter()
        .any(|argument| argument == "--check-connection");

    let lock_result = otoa_input_platform::instance_lock::acquire_instance_lock();

    let _lock = match lock_result {
        Ok(lock) => Some(lock),
        Err(error) if check_connection => {
            tracing::warn!(%error, "could not acquire instance lock for connection check; continuing");
            None
        }
        Err(_) => {
            eprintln!("otoa-input is already running");
            std::process::exit(1);
        }
    };

    let settings = load_settings()?;

    // 接続先が自分の機械で、まだ誰も待ち受けていなければ、同梱の ASR サーバーを
    // 自分で立てる。**利用者に 2 つ起動させないため。**
    // 接続確認より前に行う。ここを後にすると --check-connection が必ず失敗する。
    // 接続確認モードだけは、ここで同期に立ててから確かめる。ここを飛ばすと
    // --check-connection が必ず失敗する。GUI 起動では、モデルの自動ダウンロードで
    // 画面が出ないまま固まって見えないよう、コントローラが裏で立てる。
    if check_connection {
        if let Ok(endpoint) = deps.provider.endpoint_hint(&settings.core) {
            if let Err(failure) =
                bundled_server::start_if_needed(&endpoint.url, &settings.asr_engine, &mut |_| {})
            {
                tracing::warn!(message = %failure, "同梱の ASR サーバーを起動できない");
            }
        }
        std::process::exit(check_connection::run(&settings, deps.provider));
    }

    tracing::info!(input_gain = settings.input_gain, "input gain configured");
    match otoa_input_platform::AudioCapture::list_devices() {
        Ok(devices) => {
            for (index, device) in devices.iter().enumerate() {
                tracing::info!(
                    index,
                    name = %device.name,
                    is_default = device.is_default,
                    "input device"
                );
            }
        }
        Err(error) => tracing::warn!(%error, "failed to list input devices"),
    }
    let (to_ui, ui_updates) = unbounded_channel();
    let account_settings_available =
        deps.provider.prepare().is_some() || deps.provider.account().is_some();
    // 同梱サーバーの起動（必要ならモデルの自動ダウンロード）はコントローラが
    // 起動時に行う。失敗は readiness としてそこで持つので、ここでは渡さない。
    let runtime = wiring::start(
        settings.clone(),
        deps.provider,
        None,
        to_ui,
        account_settings_available,
    )?;
    ui::run(
        settings,
        ui_updates,
        runtime,
        deps.settings_view,
        deps.extra_settings_page,
        deps.settings_extensions,
    )
}

#[cfg(test)]
mod tests {
    use super::server_arguments;

    #[test]
    fn serve_flag_is_removed_before_forwarding_server_options() {
        let arguments = vec![
            "--port=9000".to_string(),
            "--serve".to_string(),
            "--asr-model-dir".to_string(),
            "/tmp/model".to_string(),
        ];

        assert_eq!(
            server_arguments(&arguments),
            Some(vec![
                "--port=9000".to_string(),
                "--asr-model-dir".to_string(),
                "/tmp/model".to_string(),
            ])
        );
    }

    #[test]
    fn arguments_without_serve_flag_remain_in_client_mode() {
        let arguments = vec!["--check-connection".to_string()];
        assert_eq!(server_arguments(&arguments), None);
    }
}
