//! 既に起動しているインスタンスへの「設定画面を開け」という合図。
//!
//! メニューバーが混んでいるとトレイアイコンに手が届かず、設定を開く道が
//! 塞がる。そこで **もう一度起動したら設定画面が開く** ようにする。
//! 二度目の起動はロックを取れないので、合図を置いて終わる。本体は合図を
//! 見つけたら設定画面を開く。
//!
//! 合図はソケットではなくファイルで置く。本体が監視を始める前に合図が
//! 来ても、ファイルなら消費されるまで残るので取りこぼさない。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// 合図の置き場。インスタンスロックと同じディレクトリに置く。
fn signal_path() -> PathBuf {
    dirs::runtime_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(format!(
            "{}.open-settings",
            crate::paths::app_identifier()
        ))
}

/// 「設定画面を開け」と合図する。二度目の起動側が呼ぶ。
pub fn request_open_settings() -> Result<()> {
    request_at(&signal_path())
}

/// 合図があれば消費して true。本体側が繰り返し呼ぶ。
///
/// **起動直後にも一度呼んで捨てること。** 前回の残骸で、起動するなり
/// 設定画面が開くのを防ぐ。
pub fn take_open_settings_request() -> bool {
    take_at(&signal_path())
}

fn request_at(path: &Path) -> Result<()> {
    std::fs::write(path, b"open-settings")
        .with_context(|| format!("failed to write {}", path.display()))
}

fn take_at(path: &Path) -> bool {
    std::fs::remove_file(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::{request_at, take_at};

    /// 合図を置いたら一度だけ受け取れる。二度は受け取れない。
    #[test]
    fn a_request_is_taken_exactly_once() {
        let dir = std::env::temp_dir().join("otoa-activation-test-once");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("signal");
        let _ = std::fs::remove_file(&path);

        request_at(&path).unwrap();
        assert!(take_at(&path), "置いた合図を受け取れない");
        assert!(!take_at(&path), "同じ合図を二度受け取ってしまう");
    }

    /// 合図が無ければ何も受け取らない。
    #[test]
    fn nothing_is_taken_without_a_request() {
        let dir = std::env::temp_dir().join("otoa-activation-test-none");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("signal");
        let _ = std::fs::remove_file(&path);

        assert!(!take_at(&path));
    }

    /// 二度置いても一度の消費で消える(上書きであり積み上がらない)。
    #[test]
    fn requests_collapse_into_one() {
        let dir = std::env::temp_dir().join("otoa-activation-test-collapse");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("signal");
        let _ = std::fs::remove_file(&path);

        request_at(&path).unwrap();
        request_at(&path).unwrap();
        assert!(take_at(&path));
        assert!(!take_at(&path));
    }
}
