/// 多重起動を防ぐロック。保持している間だけ有効。drop で解放される。
pub struct InstanceLock {
    #[cfg(unix)]
    #[allow(dead_code)]
    file: std::fs::File,
}

/// 取得できなければ Err。既に別のインスタンスが動いている。
pub fn acquire_instance_lock() -> anyhow::Result<InstanceLock> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;

        let lock_path = dirs::runtime_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("otoa-input.lock");
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
        // Windows has no implementation here; allowing startup is preferable to
        // pretending to provide a lock that cannot be reliably enforced.
        Ok(InstanceLock {})
    }
}
