//! 認識モデルが手元に無ければ Hugging Face から取得する。
//!
//! 配布物にモデルを同梱すると数百 MB 膨らむので、代わりに初回起動時
//! （と、設定でエンジンを変えて起動し直したとき）に自動で落とす。落とし先は
//! ユーザーごとのデータ領域 `models/<名前>` で、[`crate::bundled_server`] の
//! 探索先に含まれている。

use anyhow::{Context, Result};
use otoa_asr_server::AsrEngine;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const HF_BASE: &str = "https://huggingface.co";

/// ダウンロードの進み具合。エンジン全体の累積バイト数で表す。
#[derive(Clone, Copy)]
pub struct Progress {
    pub downloaded: u64,
    pub total: u64,
}

struct Plan {
    /// Hugging Face のリポジトリ。
    repo: &'static str,
    /// 保存先ディレクトリ名。[`crate::bundled_server`] の探索名と一致させる。
    dir_name: &'static str,
    /// `(リポジトリ上の相対パス, 保存名)`。
    ///
    /// kodama は `onnx/` の下に置かれているが、サーバーはフラットな配置を
    /// 期待するので、保存名だけ平らにする。
    files: &'static [(&'static str, &'static str)],
}

fn plan(engine: AsrEngine) -> Plan {
    match engine {
        // int8 版や safetensors は使わないので落とさない。
        AsrEngine::K2 => Plan {
            repo: "reazon-research/reazonspeech-k2-v2",
            dir_name: "reazonspeech-k2-v2",
            files: &[
                ("encoder-epoch-99-avg-1.onnx", "encoder-epoch-99-avg-1.onnx"),
                ("decoder-epoch-99-avg-1.onnx", "decoder-epoch-99-avg-1.onnx"),
                ("joiner-epoch-99-avg-1.onnx", "joiner-epoch-99-avg-1.onnx"),
                ("tokens.txt", "tokens.txt"),
            ],
        },
        AsrEngine::Kodama => Plan {
            repo: "ayousanz/kodama-ja-streaming-small",
            dir_name: "kodama-ja-streaming-small",
            files: &[
                ("onnx/encoder.onnx", "encoder.onnx"),
                ("onnx/encoder.onnx.data", "encoder.onnx.data"),
                ("onnx/cross_kv_prefill.onnx", "cross_kv_prefill.onnx"),
                ("onnx/cross_kv_prefill.onnx.data", "cross_kv_prefill.onnx.data"),
                (
                    "onnx/decoder_step_crosskv.int8a.onnx",
                    "decoder_step_crosskv.int8a.onnx",
                ),
                ("tokenizer.json", "tokenizer.json"),
            ],
        },
    }
}

fn resolve_url(repo: &str, remote: &str) -> String {
    format!("{HF_BASE}/{repo}/resolve/main/{remote}")
}

/// エンジンに必要なモデル一式を `models_root/<名前>` へ揃え、その場所を返す。
///
/// すでに正しい大きさで置いてあるファイルは飛ばすので、**途中で切れても
/// もう一度呼べば残りだけ落とす。** 各ファイルはまず `.part` へ書き、
/// 落とし切ってから正式名へ差し替えるので、中途半端なファイルが本名で
/// 残ることはない。
pub fn ensure(
    engine: AsrEngine,
    models_root: &Path,
    mut progress: impl FnMut(Progress),
) -> Result<PathBuf> {
    let plan = plan(engine);
    let dir = models_root.join(plan.dir_name);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create model directory {}", dir.display()))?;

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()
        .context("failed to build HTTP client")?;

    // 先に全ファイルの大きさを調べ、総量を出す。割合で進捗を見せるため。
    let mut sizes = Vec::with_capacity(plan.files.len());
    let mut total: u64 = 0;
    for (remote, _) in plan.files {
        let size = remote_size(&client, &resolve_url(plan.repo, remote))
            .with_context(|| format!("failed to query size of {remote}"))?;
        sizes.push(size);
        total += size;
    }

    let mut completed: u64 = 0;
    progress(Progress {
        downloaded: 0,
        total,
    });
    for ((remote, local), &size) in plan.files.iter().zip(sizes.iter()) {
        let dest = dir.join(local);
        // すでに正しい大きさで置いてあれば飛ばす。
        if fs::metadata(&dest).map(|meta| meta.len()).ok() == Some(size) {
            completed += size;
            progress(Progress {
                downloaded: completed,
                total,
            });
            continue;
        }
        download_file(
            &client,
            &resolve_url(plan.repo, remote),
            &dest,
            |written| {
                progress(Progress {
                    downloaded: completed + written,
                    total,
                });
            },
        )
        .with_context(|| format!("failed to download {remote}"))?;
        completed += size;
    }

    Ok(dir)
}

fn remote_size(client: &reqwest::blocking::Client, url: &str) -> Result<u64> {
    let response = client.head(url).send()?.error_for_status()?;
    response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .context("response had no Content-Length")
}

fn download_file(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    mut on_progress: impl FnMut(u64),
) -> Result<()> {
    let temporary = partial_path(dest);
    let mut response = client.get(url).send()?.error_for_status()?;
    let mut file = fs::File::create(&temporary)
        .with_context(|| format!("failed to create {}", temporary.display()))?;
    let mut buffer = vec![0u8; 1 << 16];
    let mut written: u64 = 0;
    loop {
        let read = response.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])?;
        written += read as u64;
        on_progress(written);
    }
    file.sync_all().ok();
    drop(file);
    fs::rename(&temporary, dest)
        .with_context(|| format!("failed to move {} into place", temporary.display()))?;
    Ok(())
}

/// `foo.onnx.data` → `foo.onnx.data.part` のように、末尾へ `.part` を足す。
///
/// `Path::with_extension` は最後の拡張子を置き換えてしまい、`encoder.onnx.data`
/// と `encoder.onnx` の一時名が衝突しうるので使わない。
fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn k2_plan_lists_the_transducer_files() {
        let plan = plan(AsrEngine::K2);
        assert_eq!(plan.dir_name, "reazonspeech-k2-v2");
        assert_eq!(plan.files.len(), 4);
        assert!(plan.files.iter().any(|(_, local)| *local == "tokens.txt"));
        // int8 版は落とさない。
        assert!(!plan.files.iter().any(|(remote, _)| remote.contains("int8")));
    }

    #[test]
    fn kodama_plan_flattens_the_onnx_subdirectory() {
        let plan = plan(AsrEngine::Kodama);
        assert_eq!(plan.dir_name, "kodama-ja-streaming-small");
        assert_eq!(plan.files.len(), 6);
        assert!(plan
            .files
            .iter()
            .any(|(remote, local)| *remote == "onnx/encoder.onnx" && *local == "encoder.onnx"));
        assert!(plan.files.iter().any(|(_, local)| *local == "tokenizer.json"));
    }

    #[test]
    fn resolve_url_points_at_the_repository_main_revision() {
        assert_eq!(
            resolve_url("a/b", "onnx/x.onnx"),
            "https://huggingface.co/a/b/resolve/main/onnx/x.onnx"
        );
    }

    // 実際に Hugging Face から小さいファイル 1 つ（k2 の tokens.txt, 約 45KB）を
    // 落として、HEAD の大きさ取得・ストリーム GET・`.part` からの差し替えまでを
    // まとめて確かめる。通信するので既定では走らせない:
    //   cargo test -p otoa-input-app -- --ignored real_download
    #[test]
    #[ignore]
    fn real_download_fetches_a_small_file() {
        let client = reqwest::blocking::Client::builder().build().unwrap();
        let url = resolve_url("reazon-research/reazonspeech-k2-v2", "tokens.txt");
        let size = remote_size(&client, &url).expect("HEAD should report a size");
        assert!(size > 0);

        let dir = std::env::temp_dir().join("otoa-download-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("tokens.txt");
        download_file(&client, &url, &dest, |_| {}).expect("download should succeed");

        assert_eq!(fs::metadata(&dest).unwrap().len(), size);
        // `.part` が残っていないこと。
        assert!(!partial_path(&dest).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_path_appends_without_dropping_extensions() {
        assert_eq!(
            partial_path(Path::new("/tmp/encoder.onnx.data")),
            PathBuf::from("/tmp/encoder.onnx.data.part")
        );
        assert_eq!(
            partial_path(Path::new("/tmp/tokens.txt")),
            PathBuf::from("/tmp/tokens.txt.part")
        );
    }
}
