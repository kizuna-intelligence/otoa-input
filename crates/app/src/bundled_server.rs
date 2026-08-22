//! 同梱の ASR サーバーを、必要なときだけ自分で起動する。
//!
//! **利用者に 2 つのプロセスを起動させないため**にある。実行ファイルを
//! ダブルクリックしただけで、認識サーバーごと立ち上がってほしい。
//!
//! 起動するのは次を全部満たすときだけ。
//!
//! 1. 接続先が自分の機械（localhost）である
//! 2. そのポートで誰も待ち受けていない
//! 3. 認識モデルが見つかる
//!
//! **既に誰かが待ち受けているなら起動しない。** 自分で立てたサーバーや、
//! 別の機械のサーバーを二重に立ち上げないためである。

use std::{
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    time::Duration,
};

/// 認識モデルを探す場所。**先に見つかった方を使う。**
///
/// 実行ファイルの隣を先に見るのは、配布物を展開した場所に置いた分を
/// 優先させるためである。
fn model_directory() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("models/reazonspeech-k2-v2"));
            // macOS の .app では実行ファイルが Contents/MacOS に入る。
            if let Some(contents) = parent.parent() {
                candidates.push(contents.join("Resources/models/reazonspeech-k2-v2"));
            }
        }
    }
    if let Some(data) = dirs_data_directory() {
        candidates.push(data.join("models/reazonspeech-k2-v2"));
    }
    candidates.push(PathBuf::from("models/reazonspeech-k2-v2"));
    candidates
        .into_iter()
        .find(|path| path.join("tokens.txt").is_file())
}

fn dirs_data_directory() -> Option<PathBuf> {
    otoa_input_platform::data_directory().ok()
}

/// `url` が自分の機械を指しているなら、その待ち受けアドレスを返す。
fn local_address(url: &str) -> Option<SocketAddr> {
    let rest = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("wss://"))?;
    let host_port = rest.split('/').next()?;
    let (host, port) = host_port.rsplit_once(':')?;
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1") {
        return None;
    }
    format!("127.0.0.1:{port}").parse().ok()
}

fn is_listening(address: SocketAddr) -> bool {
    TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
}

/// 同梱サーバーを起動する必要があるか調べ、必要なら起動する。
///
/// 起動できなかった理由は戻り値で返す。**起動しないこと自体は失敗ではない。**
/// 別の機械のサーバーを使う構成なら、これは正常な状態である。
pub fn start_if_needed(server_url: &str) -> Result<Option<PathBuf>, String> {
    let Some(address) = local_address(server_url) else {
        return Ok(None); // 自分の機械ではない
    };
    if is_listening(address) {
        tracing::info!(%address, "ASR サーバーは既に動いているので起動しない");
        return Ok(None);
    }
    let Some(model_dir) = model_directory() else {
        return Err(
            "認識モデルが見つかりません。README の手順で reazonspeech-k2-v2 を取得し、\
             実行ファイルの隣の models/reazonspeech-k2-v2 に置いてください。"
                .to_string(),
        );
    };

    let port = address.port();
    let model = model_dir.clone();
    std::thread::Builder::new()
        .name("otoa-asr-server".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!(%error, "ASR サーバーの実行環境を作れません");
                    return;
                }
            };
            let config = otoa_asr_server::Config {
                port,
                asr_model_dir: model,
                ..otoa_asr_server::Config::default()
            };
            if let Err(error) = runtime.block_on(otoa_asr_server::run(config)) {
                tracing::error!(%error, "ASR サーバーが停止しました");
            }
        })
        .map_err(|error| format!("ASR サーバーを起動できません: {error}"))?;

    // **待ち受け開始まで待つ。** モデルの読み込みに数秒かかるので、
    // ここで待たないと最初の接続が必ず失敗する。
    wait_until_listening(address, Duration::from_secs(120))
        .map_err(|error| format!("ASR サーバーが待ち受けを始めません: {error}"))?;

    Ok(Some(model_dir))
}

fn wait_until_listening(address: SocketAddr, limit: Duration) -> Result<(), String> {
    let started = std::time::Instant::now();
    let mut logged = false;
    while started.elapsed() < limit {
        if is_listening(address) {
            return Ok(());
        }
        if !logged && started.elapsed() > Duration::from_secs(2) {
            tracing::info!("認識モデルを読み込んでいます…");
            logged = true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!("{} 秒待っても応答しません", limit.as_secs()))
}
