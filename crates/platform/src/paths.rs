use anyhow::{Context, Result};
use std::{path::PathBuf, sync::OnceLock};

/// 設定とデータを置くディレクトリ名。
///
/// **配布ごとに変えるためにある。** 同じ名前を使うと、設定ファイルを共有して
/// しまい、片方で設定した接続先をもう片方が引き継ぐ。認証情報を持たない版が
/// 外部へ繋ごうとして失敗する、という形で表に出る。
static APP_DIRECTORY: OnceLock<String> = OnceLock::new();

const DEFAULT_APP_DIRECTORY: &str = "otoa-input";

/// ディレクトリ名を決める。**設定を読む前に一度だけ呼ぶ。**
///
/// 二度目以降は無視される。読み込みの後に変えられると、どちらの設定を
/// 使っているのか追えなくなるためである。
pub fn set_app_directory(name: &str) {
    let _ = APP_DIRECTORY.set(name.to_string());
}

/// 設定・データ・インスタンスロックを配布ごとに分ける識別名。
pub(crate) fn app_identifier() -> &'static str {
    APP_DIRECTORY
        .get()
        .map(String::as_str)
        .unwrap_or(DEFAULT_APP_DIRECTORY)
}

/// 設定ファイルのパス。親ディレクトリは作る。
pub fn settings_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("failed to determine config directory")?;
    let app_dir = config_dir.join(app_identifier());
    std::fs::create_dir_all(&app_dir)
        .with_context(|| format!("failed to create settings directory {}", app_dir.display()))?;
    Ok(app_dir.join("settings.json"))
}

/// 認識モデルなど、大きめのデータを置く場所。
/// Linux なら `~/.local/share/<ディレクトリ名>`。
pub fn data_directory() -> Result<PathBuf> {
    let base = dirs::data_dir().context("failed to determine data directory")?;
    let directory = base.join(app_identifier());
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create data directory {}", directory.display()))?;
    Ok(directory)
}
