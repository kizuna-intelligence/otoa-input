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
//! otoa_input_app::run(otoa_input_app::Deps { provider })
//! # }
//! ```

use crossbeam_channel::unbounded as unbounded_channel;
use otoa_input_core::ConnectionProvider;
use otoa_input_platform::{PasteMethod, TextOutput};
use std::sync::Arc;

mod bundled_server;
mod check_connection;
mod connection;
mod controller;
mod settings;
mod settings_io;
mod ui;
mod wiring;

pub use connection::SelfHostedProvider;
pub use settings::Settings;

/// [`run`] に差し込むもの。
///
/// **拡張点はここ 1 つだけにしてある。** 増やすたびに、公開側が永久に
/// 維持しなければならない API が増える。接続先ごとの設定は
/// [`Settings::product`] を通して各実装が自分で解釈する。
pub struct Deps {
    /// 接続先の解決、認証、アカウント表示を担う。
    ///
    /// [`ConnectionProvider::prepare`] が `None` を返す実装では、
    /// ログイン関係の UI（トレイの項目、設定画面のアカウント欄）は出ない。
    pub provider: Arc<dyn ConnectionProvider>,
}

/// 設定ファイルを読む。[`Deps`] を組み立てるために先に必要になる。
pub fn load_settings() -> anyhow::Result<Settings> {
    settings_io::load()
}

/// 同梱の [`SelfHostedProvider`] で起動する。
pub fn run_self_hosted() -> anyhow::Result<()> {
    // **設定を読む前に呼ぶ。** 他の配布と設定ファイルを共有しないため。
    otoa_input_platform::set_app_directory("otoa-input-oss");
    run(Deps {
        provider: Arc::new(SelfHostedProvider),
    })
}

/// `--help` の本文。オプションを増やしたらここも足す。
pub const USAGE: &str = "\
otoa-input — 話した内容をカーソル位置へ貼り付ける音声入力

使い方:
  otoa-input [オプション]

オプション:
  --serve             ASR サーバーだけを動かす（画面は出さない）
  --check-connection  接続先へ繋いで結果を表示し、終了する
  --paste-test [文字列]
                      貼り付けだけを 1 回試して終了する。
                      文字を入れたい場所にカーソルを置いてから実行する
  -h, --help          この使い方を表示する

設定はトレイアイコンの「設定」から変更する。設定ファイルの場所は
Linux なら ~/.config/otoa-input-oss/settings.json。

接続先が空のときは、同梱の otoa-asr-server を既定の設定で起動した
アドレス (ws://127.0.0.1:8770/asr/v1) へ繋ぐ。
";

/// 貼り付けだけを 1 回試す。
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
    output.poll_paste_target();
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

/// クライアントを起動する。`--check-connection` が渡されていれば接続確認だけ行う。
pub fn run(deps: Deps) -> anyhow::Result<()> {
    otoa_input_platform::console::use_utf8_output();
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    // 使い方の表示は、多重起動の判定より先に行う。あとに置くと、
    // 起動中の `--help` が「already running」で終わる。
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        print!("{USAGE}");
        return Ok(());
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
    if let Ok(endpoint) = deps.provider.endpoint(&settings.core) {
        match bundled_server::start_if_needed(&endpoint.url, &settings.asr_engine) {
            Ok(Some(model_dir)) => {
                tracing::info!(model_dir = %model_dir.display(), "同梱の ASR サーバーを起動した")
            }
            Ok(None) => {}
            // 起動できなくても続ける。設定画面でエンジンや接続先を直せるようにする。
            Err(message) => tracing::warn!(%message, "同梱の ASR サーバーを起動できない"),
        }
    }

    if check_connection {
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
    let runtime = wiring::start(settings.clone(), deps.provider, to_ui)?;
    ui::run(settings, ui_updates, runtime)
}
