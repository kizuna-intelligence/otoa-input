/// 多重起動を防ぐロック。保持している間だけ有効。drop で解放される。
pub struct InstanceLock {
    #[cfg(unix)]
    #[allow(dead_code)]
    file: std::fs::File,
}

fn lock_path() -> std::path::PathBuf {
    dirs::runtime_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(lock_file_name(crate::paths::app_identifier()))
}

fn lock_file_name(app_identifier: &str) -> String {
    format!("{app_identifier}.lock")
}

/// 取得できなければ Err。既に別のインスタンスが動いている。
pub fn acquire_instance_lock() -> anyhow::Result<InstanceLock> {
    // Windows でも同じ識別規則を使う。現在 Windows ではファイルロック自体を
    // 実装していないが、実装を追加するときに固定名へ戻らないよう経路を共通にする。
    let lock_path = lock_path();

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;

        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)?;

        // SAFETY: `file` remains open for the lifetime of the returned lock.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == -1 {
            anyhow::bail!("could not acquire instance lock at {}", lock_path.display());
        }

        Ok(InstanceLock { file })
    }

    #[cfg(not(unix))]
    {
        let _ = lock_path;
        // Windows has no implementation here; allowing startup is preferable to
        // pretending to provide a lock that cannot be reliably enforced.
        Ok(InstanceLock {})
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn lock_name_follows_distribution_identifier() {
        assert_eq!(super::lock_file_name("otoa-input"), "otoa-input.lock");
        assert_eq!(
            super::lock_file_name("otoa-input-oss"),
            "otoa-input-oss.lock"
        );
    }
}
