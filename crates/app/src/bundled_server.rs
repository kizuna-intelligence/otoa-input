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

use otoa_asr_server::AsrEngine;
use std::{
    fmt,
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::Duration,
};

/// 同梱サーバーを起動できなかった理由。
///
/// ログには探索先を含む詳しい情報を残す一方、readiness には限られた表示幅で
/// 次の行動が分かる短い文だけを渡す。
pub(crate) struct StartupFailure {
    log_message: String,
    readiness_message: String,
}

impl StartupFailure {
    pub(crate) fn into_readiness_message(self) -> String {
        self.readiness_message
    }
}

impl fmt::Display for StartupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.log_message)
    }
}

fn model_details(engine: AsrEngine) -> (&'static str, &'static str) {
    match engine {
        AsrEngine::K2 => ("reazonspeech-k2-v2", "tokens.txt"),
        AsrEngine::Kodama => ("kodama-ja-streaming-small", "tokenizer.json"),
    }
}

/// 認識モデルを探す場所。**先に見つかった方を使う。**
///
/// 実行ファイルの隣を先に見るのは、配布物を展開した場所に置いた分を
/// 優先させるためである。
fn model_directories(engine: AsrEngine) -> Vec<PathBuf> {
    let (directory_name, _) = model_details(engine);
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("models").join(directory_name));
            // macOS の .app では実行ファイルが Contents/MacOS に入る。
            if let Some(contents) = parent.parent() {
                candidates.push(contents.join("Resources/models").join(directory_name));
            }
        }
    }
    if let Some(data) = dirs_data_directory() {
        candidates.push(data.join("models").join(directory_name));
    }
    candidates.push(PathBuf::from("models").join(directory_name));
    candidates
}

fn model_directory(engine: AsrEngine, candidates: &[PathBuf]) -> Option<PathBuf> {
    let (_, marker_file) = model_details(engine);
    candidates
        .iter()
        .find(|path| path.join(marker_file).is_file())
        .cloned()
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

fn startup_error(
    engine: &str,
    candidates: &[PathBuf],
    reason: &str,
    readiness_message: String,
) -> StartupFailure {
    let searched = if candidates.is_empty() {
        "（認識エンジンが未知のため、モデルディレクトリは探索していません）".to_string()
    } else {
        candidates
            .iter()
            .map(|path| format!("- {}", path.display()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    StartupFailure {
        log_message: format!(
            "同梱の ASR サーバーを起動できません: {reason}\n\
             選択中の認識エンジン: {engine}\n\
             モデルの探索先:\n{searched}\n\
             アプリはこのまま起動します。設定画面から reazonspeech または kodama を選べます。"
        ),
        readiness_message,
    }
}

/// 認識モデルを自動ダウンロードし、その置き場所を返す。
///
/// 落とし先はユーザーのデータ領域 `models/<名前>`。ここは
/// [`model_directories`] の探索先に含まれているので、次回以降は
/// ダウンロードせずに見つかる。
fn download_model(
    engine: AsrEngine,
    engine_name: &str,
    candidates: &[PathBuf],
    progress: &mut dyn FnMut(crate::model_download::Progress),
) -> Result<PathBuf, StartupFailure> {
    let (directory_name, _) = model_details(engine);
    let Some(models_root) = dirs_data_directory().map(|data| data.join("models")) else {
        let reason = format!("認識モデル {directory_name} の保存先（データ領域）が分かりません。");
        return Err(startup_error(
            engine_name,
            candidates,
            &reason,
            reason.clone(),
        ));
    };
    tracing::info!(
        engine = engine_name,
        model = directory_name,
        "認識モデルが見つからないので自動ダウンロードします"
    );
    crate::model_download::ensure(engine, &models_root, |status| progress(status)).map_err(
        |error| {
            let reason =
                format!("認識モデル {directory_name} の自動ダウンロードに失敗しました: {error:#}");
            startup_error(
                engine_name,
                candidates,
                &reason,
                "認識モデルのダウンロードに失敗しました。通信環境を確かめて起動し直してください。"
                    .to_string(),
            )
        },
    )
}

/// 同梱サーバーを起動する必要があるか調べ、必要なら起動する。
///
/// 起動できなかった理由は戻り値で返す。**起動しないこと自体は失敗ではない。**
/// 別の機械のサーバーを使う構成なら、これは正常な状態である。
pub(crate) fn start_if_needed(
    server_url: &str,
    engine: &str,
    progress: &mut dyn FnMut(crate::model_download::Progress),
) -> Result<Option<PathBuf>, StartupFailure> {
    let Some(address) = local_address(server_url) else {
        return Ok(None); // 自分の機械ではない
    };
    let engine_name = engine;
    let engine = engine.parse::<AsrEngine>().map_err(|error| {
        startup_error(
            engine_name,
            &[],
            &error,
            "認識エンジンを選べません。設定から reazonspeech または kodama を選んでください。"
                .to_string(),
        )
    })?;
    if is_listening(address) {
        tracing::info!(%address, "ASR サーバーは既に動いているので起動しない");
        return Ok(None);
    }
    let candidates = model_directories(engine);
    // 見つからなければ、その場で落としてくる。同梱すると配布物が数百 MB
    // 膨らむので、初回だけ Hugging Face から取る。落とし先はユーザーの
    // データ領域で、上の探索先に含まれている。
    let model_dir = match model_directory(engine, &candidates) {
        Some(model_dir) => model_dir,
        None => download_model(engine, engine_name, &candidates, progress)?,
    };

    let port = address.port();
    let model = model_dir.clone();
    let (startup_result_tx, startup_result_rx) = mpsc::sync_channel(1);
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
                    let _ = startup_result_tx
                        .send(Err(format!("ASR サーバーの実行環境を作れません: {error}")));
                    return;
                }
            };
            let config = otoa_asr_server::Config {
                port,
                asr_engine: engine,
                asr_model_dir: model,
                pseudo_stream: engine == AsrEngine::Kodama,
                ..otoa_asr_server::Config::default()
            };
            let result = match runtime.block_on(otoa_asr_server::run(config)) {
                Ok(()) => Err("ASR サーバーが待ち受け開始前に停止しました".to_string()),
                Err(error) => {
                    tracing::error!(%error, "ASR サーバーが停止しました");
                    Err(format!("{error:#}"))
                }
            };
            let _ = startup_result_tx.send(result);
        })
        .map_err(|error| {
            startup_error(
                engine_name,
                &candidates,
                &format!("ASR サーバーのスレッドを起動できません: {error}"),
                "同梱の ASR サーバー用スレッドを起動できません。詳しい理由はログを確認してください。"
                    .to_string(),
            )
        })?;

    // **待ち受け開始まで待つ。** モデルの読み込みに数秒かかるので、
    // ここで待たないと最初の接続が必ず失敗する。
    wait_until_listening(address, Duration::from_secs(120), &startup_result_rx)
        .map_err(|error| {
            startup_error(
                engine_name,
                &candidates,
                &error,
                "同梱の ASR サーバーが待ち受けを開始できません。詳しい理由はログを確認してください。"
                    .to_string(),
            )
        })?;

    Ok(Some(model_dir))
}

fn wait_until_listening(
    address: SocketAddr,
    limit: Duration,
    startup_result: &Receiver<Result<(), String>>,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    let mut logged = false;
    while started.elapsed() < limit {
        match startup_result.try_recv() {
            Ok(result) => return result,
            Err(TryRecvError::Disconnected) => {
                return Err("ASR サーバーのスレッドが起動結果を返さず終了しました".to_string());
            }
            Err(TryRecvError::Empty) => {}
        }
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

#[cfg(test)]
mod tests {
    use super::{start_if_needed, startup_error, wait_until_listening};
    use std::{net::TcpListener, path::PathBuf, sync::mpsc, time::Duration};

    #[test]
    fn wait_returns_server_error_without_waiting_for_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve test port");
        let address = listener.local_addr().expect("test address");
        drop(listener);
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Err("actual model load error".to_string()))
            .expect("send startup result");

        let error = wait_until_listening(address, Duration::from_secs(120), &receiver)
            .expect_err("server startup should fail");

        assert_eq!(error, "actual model load error");
    }

    #[test]
    fn unknown_engine_warns_even_when_local_server_is_already_listening() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen on test port");
        let address = listener.local_addr().expect("test address");

        let error = start_if_needed(
            &format!("ws://{address}/asr/v1"),
            "misspelled-engine",
            &mut |_| {},
        )
        .expect_err("unknown engine should be reported");

        let log_message = error.to_string();
        assert!(log_message.contains("選択中の認識エンジン: misspelled-engine"));
        assert!(log_message.contains("モデルディレクトリは探索していません"));
        assert!(log_message.contains("設定画面から reazonspeech または kodama を選べます"));
        assert_eq!(
            error.readiness_message,
            "認識エンジンを選べません。設定から reazonspeech または kodama を選んでください。"
        );
    }

    #[test]
    fn startup_failure_keeps_search_paths_out_of_readiness_message() {
        let candidates = vec![PathBuf::from(
            "/very/long/private/path/to/kodama-ja-streaming-small",
        )];
        let readiness_message = "認識モデル kodama-ja-streaming-small が見つかりません。設定から認識エンジンを選び直してください。";
        let error = startup_error(
            "kodama",
            &candidates,
            "model not found",
            readiness_message.to_string(),
        );

        assert!(error.to_string().contains(candidates[0].to_str().unwrap()));
        assert_eq!(error.readiness_message, readiness_message);
        assert!(!error.readiness_message.contains("/very/long/private/path"));
        assert!(error.readiness_message.chars().count() <= 80);
    }
}
