use crate::settings::Settings;
use anyhow::{Context, Result};
use otoa_input_platform::settings_path;
use std::fs::OpenOptions;
use std::io::Write;

pub fn load() -> Result<Settings> {
    let path = settings_path()?;
    match std::fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents)
            .with_context(|| format!("failed to parse settings {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(error) => {
            Err(error).with_context(|| format!("failed to read settings {}", path.display()))
        }
    }
}

pub fn save(settings: &Settings) -> Result<()> {
    let path = settings_path()?;
    let contents =
        serde_json::to_string_pretty(settings).context("failed to serialize settings")?;
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("failed to open settings {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write settings {}", path.display()))?;
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
        .with_context(|| format!("failed to protect settings {}", path.display()))?;
    Ok(())
}
