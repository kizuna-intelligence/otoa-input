use anyhow::{Context, Result};
use std::thread;
use std::time::{Duration, Instant};
use tracing::debug;

#[cfg(target_os = "linux")]
use std::{
    io::Write,
    process::{Command, Stdio},
};

#[cfg(any(target_os = "windows", target_os = "macos"))]
use enigo::{
    Direction::{Click, Press, Release},
    Enigo, Key, Keyboard, Settings,
};

const MIN_EMIT_INTERVAL_MS: u64 = 150;

#[cfg(target_os = "linux")]
const XCLIP_MAX_RETRIES: usize = 3;

#[derive(Debug)]
pub enum PasteMethod {
    /// クリップボードに置くだけ。ユーザーが自分で貼る。
    ClipboardOnly,
    /// クリップボードに置き、Ctrl+V / Cmd+V を送る。
    ClipboardAndPaste,
}

pub struct TextOutput {
    last_emit: Option<Instant>,
    #[cfg(target_os = "linux")]
    pid: u32,
    #[cfg(target_os = "linux")]
    last_target: Option<u64>,
    #[cfg(target_os = "linux")]
    clipboard: Option<arboard::Clipboard>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    clipboard: arboard::Clipboard,
}

impl TextOutput {
    pub fn new() -> Result<Self> {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let clipboard = arboard::Clipboard::new().context("failed to initialize clipboard")?;

        #[cfg(target_os = "linux")]
        let pid = std::process::id();

        #[cfg(target_os = "linux")]
        {
            let xclip_available = command_available("xclip");
            let xsel_available = command_available("xsel");
            let wl_copy_available = command_available("wl-copy");
            let xdotool_available = command_available("xdotool");
            tracing::info!(
                pid,
                xclip = xclip_available,
                xsel = xsel_available,
                wl_copy = wl_copy_available,
                xdotool = xdotool_available,
                "text output command availability"
            );
        }

        let output = Self {
            last_emit: None,
            #[cfg(target_os = "linux")]
            pid,
            #[cfg(target_os = "linux")]
            last_target: None,
            #[cfg(target_os = "linux")]
            clipboard: None,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            clipboard,
        };

        #[cfg(target_os = "linux")]
        let output = Self::prime_paste_target(output);

        Ok(output)
    }

    /// テキストを出力する。`ClipboardAndPaste` では貼り付けキーも送る。
    pub fn emit(&mut self, text: &str, method: PasteMethod) -> Result<()> {
        debug!("emit start len={} method={:?}", text.len(), method);
        self.wait_for_emit_interval();

        let result = {
            #[cfg(target_os = "linux")]
            {
                self.emit_linux(text, method)
            }

            #[cfg(any(target_os = "windows", target_os = "macos"))]
            {
                (|| -> Result<()> {
                    let clipboard_result = self.clipboard.set_text(text.to_string());
                    match &clipboard_result {
                        Ok(()) => debug!("clipboard write method=arboard ok=true"),
                        Err(error) => {
                            tracing::warn!("clipboard write method=arboard ok=false error={error}")
                        }
                    }
                    clipboard_result.context("failed to set clipboard text")?;
                    if matches!(&method, PasteMethod::ClipboardOnly) {
                        Ok(())
                    } else {
                        thread::sleep(Duration::from_millis(80));
                        send_paste_shortcut()
                    }
                })()
            }

            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            {
                let _ = (text, method);
                Ok(())
            }
        };
        let ok = result.is_ok();
        debug!("emit done ok={ok}");
        result
    }

    #[cfg(target_os = "linux")]
    pub fn poll_paste_target(&mut self) {
        self.refresh_paste_target();
    }

    #[cfg(not(target_os = "linux"))]
    pub fn poll_paste_target(&mut self) {}

    fn wait_for_emit_interval(&mut self) {
        let minimum = Duration::from_millis(MIN_EMIT_INTERVAL_MS);
        if let Some(last_emit) = self.last_emit {
            let elapsed = last_emit.elapsed();
            if elapsed < minimum {
                thread::sleep(minimum - elapsed);
            }
        }
        self.last_emit = Some(Instant::now());
    }

    #[cfg(target_os = "linux")]
    fn prime_paste_target(mut output: Self) -> Self {
        output.refresh_paste_target();
        output
    }

    #[cfg(target_os = "linux")]
    fn refresh_paste_target(&mut self) {
        let Ok(window) = get_active_window() else {
            return;
        };
        let Ok(window_pid) = get_window_pid(window) else {
            return;
        };

        if window_pid == self.pid {
            return;
        }
        if self.last_target != Some(window) {
            debug!("paste target updated window={} pid={}", window, window_pid);
        }
        self.last_target = Some(window);
    }

    #[cfg(target_os = "linux")]
    fn emit_linux(&mut self, text: &str, method: PasteMethod) -> Result<()> {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            self.emit_wayland(text, method)
        } else {
            self.emit_x11(text, method)
        }
    }

    #[cfg(target_os = "linux")]
    fn emit_wayland(&mut self, text: &str, method: PasteMethod) -> Result<()> {
        if !command_available("wl-copy") {
            tracing::warn!(
                "clipboard write method=wl-copy ok=false reason=unavailable; leaving transcript in the clipboard without paste"
            );
            self.set_arboard_fallback(text)?;
            return Ok(());
        }

        match write_clipboard_command("wl-copy", &[], text) {
            Ok(()) => debug!("clipboard write method=wl-copy ok=true"),
            Err(error) => {
                tracing::warn!("clipboard write method=wl-copy ok=false error={error:#}");
                return Err(error.context("failed to set the Wayland clipboard with wl-copy"));
            }
        }
        if matches!(&method, PasteMethod::ClipboardOnly) {
            return Ok(());
        }

        let command = "wtype -M ctrl v -m ctrl";
        debug!("paste key command={command} start");
        match Command::new("wtype")
            .args(["-M", "ctrl", "v", "-m", "ctrl"])
            .status()
        {
            Ok(status) if status.success() => {
                debug!("paste key command={command} ok=true");
                Ok(())
            }
            Ok(status) => {
                tracing::warn!(
                    "paste key command={command} ok=false status={status}; leaving transcript in clipboard"
                );
                Ok(())
            }
            Err(error) => {
                tracing::warn!(
                    "paste key command={command} ok=false error={error}; leaving transcript in clipboard"
                );
                Ok(())
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn emit_x11(&mut self, text: &str, method: PasteMethod) -> Result<()> {
        let clipboard_ready = if command_available("xclip") {
            if set_and_verify_xclip(text) {
                true
            } else {
                tracing::warn!(
                    "xclip clipboard verification failed; skipping Ctrl+V for this transcript"
                );
                false
            }
        } else if command_available("xsel") {
            match write_clipboard_command("xsel", &["--clipboard", "--input"], text) {
                Ok(()) => {
                    debug!("clipboard write method=xsel ok=true");
                    true
                }
                Err(error) => {
                    tracing::warn!(
                        "clipboard write method=xsel ok=false error={error:#}; skipping Ctrl+V for this transcript"
                    );
                    false
                }
            }
        } else {
            tracing::warn!(
                "clipboard write method=arboard reason=xclip-and-xsel-unavailable; using arboard clipboard fallback on X11"
            );
            self.set_arboard_fallback(text)?;
            true
        };

        if !clipboard_ready || matches!(&method, PasteMethod::ClipboardOnly) {
            return Ok(());
        }
        self.send_paste_to_target()
    }

    #[cfg(target_os = "linux")]
    fn set_arboard_fallback(&mut self, text: &str) -> Result<()> {
        if self.clipboard.is_none() {
            match arboard::Clipboard::new().context("failed to initialize clipboard fallback") {
                Ok(clipboard) => self.clipboard = Some(clipboard),
                Err(error) => {
                    tracing::warn!("clipboard write method=arboard ok=false error={error:#}");
                    return Err(error);
                }
            }
        }
        let result = self
            .clipboard
            .as_mut()
            .context("clipboard fallback is not initialized")?
            .set_text(text.to_string())
            .context("failed to set clipboard text with arboard");
        match &result {
            Ok(()) => debug!("clipboard write method=arboard ok=true"),
            Err(error) => {
                tracing::warn!("clipboard write method=arboard ok=false error={error:#}")
            }
        }
        result
    }

    #[cfg(target_os = "linux")]
    fn send_paste_to_target(&mut self) -> Result<()> {
        self.refresh_paste_target();

        let active_window = match get_active_window() {
            Ok(window) => window,
            Err(error) => {
                tracing::warn!(
                    "paste target lookup command=xdotool getactivewindow ok=false error={error:#}"
                );
                return Ok(());
            }
        };
        let active_pid = match get_window_pid(active_window) {
            Ok(pid) => pid,
            Err(error) => {
                tracing::warn!(
                    "paste target lookup command=xdotool getwindowpid ok=false window={} error={error:#}",
                    active_window
                );
                return Ok(());
            }
        };

        debug!(
            "paste active window={} pid={} own_pid={} target={:?}",
            active_window, active_pid, self.pid, self.last_target
        );
        if active_pid != self.pid {
            return send_paste_shortcut();
        }

        let Some(target) = self.last_target else {
            tracing::warn!("no paste target");
            return Ok(());
        };
        activate_paste_target(target)?;
        send_paste_shortcut()
    }
}

#[cfg(target_os = "linux")]
fn command_available(command: &str) -> bool {
    Command::new(command)
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

#[cfg(target_os = "linux")]
fn write_clipboard_command(command: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {command}"))?;
    let mut stdin = child
        .stdin
        .take()
        .with_context(|| format!("{command} did not provide stdin"))?;
    stdin
        .write_all(text.as_bytes())
        .with_context(|| format!("failed to write transcript to {command} stdin"))?;
    drop(stdin);

    let status = child
        .wait()
        .with_context(|| format!("failed to wait for {command}"))?;
    anyhow::ensure!(
        status.success(),
        "{command} clipboard command failed: {status}"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn read_clipboard_command(command: &str, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new(command)
        .args(args)
        .output()
        .with_context(|| format!("failed to read clipboard with {command}"))?;
    anyhow::ensure!(
        output.status.success(),
        "{command} clipboard read failed: {}",
        output.status
    );
    Ok(output.stdout)
}

#[cfg(target_os = "linux")]
fn set_and_verify_xclip(text: &str) -> bool {
    let total_attempts = XCLIP_MAX_RETRIES + 1;
    let mut final_result = "error";
    for attempt in 1..=total_attempts {
        match write_clipboard_command("xclip", &["-selection", "clipboard"], text) {
            Ok(()) => debug!(
                "clipboard write method=xclip ok=true attempt={} retries={}",
                attempt,
                attempt - 1
            ),
            Err(error) => {
                final_result = "error";
                tracing::warn!(
                    "clipboard write method=xclip ok=false attempt={} retries={} error={error:#}",
                    attempt,
                    attempt - 1
                );
                if attempt < total_attempts {
                    debug!(
                        "clipboard verification retry method=xclip next_attempt={} retries={}",
                        attempt + 1,
                        attempt
                    );
                }
                continue;
            }
        }

        let verified = match read_clipboard_command("xclip", &["-o", "-selection", "clipboard"]) {
            Ok(output) => {
                let matched = output == text.as_bytes();
                final_result = if matched { "match" } else { "mismatch" };
                debug!(
                    "clipboard readback method=xclip result={} attempt={} retries={}",
                    if matched { "match" } else { "mismatch" },
                    attempt,
                    attempt - 1
                );
                matched
            }
            Err(error) => {
                final_result = "error";
                tracing::warn!(
                    "clipboard readback method=xclip result=error attempt={} retries={} error={error:#}",
                    attempt,
                    attempt - 1
                );
                false
            }
        };
        if verified {
            return true;
        }
        if attempt < total_attempts {
            debug!(
                "clipboard verification retry method=xclip next_attempt={} retries={}",
                attempt + 1,
                attempt
            );
        }
    }
    tracing::warn!(
        "clipboard readback method=xclip result={} retries={}",
        final_result,
        XCLIP_MAX_RETRIES
    );
    false
}

#[cfg(target_os = "linux")]
fn get_active_window() -> Result<u64> {
    let output = Command::new("xdotool")
        .arg("getactivewindow")
        .output()
        .context("failed to execute xdotool getactivewindow")?;
    anyhow::ensure!(
        output.status.success(),
        "xdotool getactivewindow failed: {}",
        output.status
    );
    parse_first_number(&output.stdout, "xdotool getactivewindow returned no window")
}

#[cfg(target_os = "linux")]
fn get_window_pid(window: u64) -> Result<u32> {
    let window_arg = window.to_string();
    if let Ok(output) = Command::new("xdotool")
        .args(["getwindowpid", window_arg.as_str()])
        .output()
    {
        if output.status.success() {
            if let Ok(pid) = parse_first_number(&output.stdout, "xdotool returned no window PID") {
                if let Ok(pid) = u32::try_from(pid) {
                    return Ok(pid);
                }
            }
        }
    }

    let output = Command::new("xprop")
        .args(["-id", window_arg.as_str(), "_NET_WM_PID"])
        .output()
        .context("failed to execute xprop for _NET_WM_PID")?;
    anyhow::ensure!(
        output.status.success(),
        "xprop _NET_WM_PID failed: {}",
        output.status
    );
    let pid = parse_first_number(&output.stdout, "xprop returned no _NET_WM_PID")?;
    u32::try_from(pid).context("_NET_WM_PID does not fit in a process ID")
}

#[cfg(target_os = "linux")]
fn parse_first_number(output: &[u8], error_message: &str) -> Result<u64> {
    let output = std::str::from_utf8(output).context("window command returned invalid UTF-8")?;
    output
        .split_whitespace()
        .find_map(|token| token.parse::<u64>().ok())
        .context(error_message.to_string())
}

#[cfg(target_os = "linux")]
fn activate_paste_target(window: u64) -> Result<()> {
    let window_arg = window.to_string();
    let command = format!("xdotool windowactivate --sync {window}");
    debug!("paste target activate command={command} start");
    let status = Command::new("xdotool")
        .args(["windowactivate", "--sync", window_arg.as_str()])
        .status()
        .context("failed to execute xdotool windowactivate")?;
    if status.success() {
        debug!("paste target activate command={command} ok=true");
        Ok(())
    } else {
        tracing::warn!("paste target activate command={command} ok=false status={status}");
        anyhow::bail!("{command} failed: {status}");
    }
}

#[cfg(target_os = "linux")]
struct CtrlKeyupGuard {
    armed: bool,
}

#[cfg(target_os = "linux")]
impl CtrlKeyupGuard {
    fn new() -> Self {
        Self { armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(target_os = "linux")]
impl Drop for CtrlKeyupGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Err(error) = run_xdotool_key_step("keyup ctrl (cleanup)", "keyup", &["ctrl"]) {
            tracing::error!("paste key step=keyup ctrl (cleanup) ok=false error={error:#}");
        }
    }
}

#[cfg(target_os = "linux")]
fn run_xdotool_key_step(step: &str, action: &str, keys: &[&str]) -> Result<()> {
    let command = format!("xdotool {action} {}", keys.join(" "));
    let mut args = Vec::with_capacity(keys.len() + 1);
    args.push(action);
    args.extend_from_slice(keys);

    debug!("paste key step={step} command={command} start");
    match Command::new("xdotool").args(args).status() {
        Ok(status) => {
            debug!(
                "paste key step={step} command={command} ok={} status={status} exit_code={:?}",
                status.success(),
                status.code()
            );
            if status.success() {
                return Ok(());
            }
            tracing::warn!(
                "paste key step={step} command={command} ok=false status={status} exit_code={:?}",
                status.code()
            );
        }
        Err(error) => {
            tracing::warn!(
                "paste key step={step} command={command} ok=false exit_code=unavailable error={error}"
            );
            return Err(error)
                .with_context(|| format!("failed to execute paste key step {step}: {command}"));
        }
    }
    anyhow::bail!("paste key step {step} failed: {command}");
}

#[cfg(target_os = "linux")]
fn send_paste_shortcut() -> Result<()> {
    let mut ctrl_keyup_guard = CtrlKeyupGuard::new();
    let result = run_xdotool_key_step(
        "keyup ctrl shift alt super keydown ctrl keydown v keyup v keyup ctrl",
        "keyup",
        &[
            "ctrl", "shift", "alt", "super", "keydown", "ctrl", "keydown", "v", "keyup", "v",
            "keyup", "ctrl",
        ],
    );
    if result.is_ok() {
        ctrl_keyup_guard.disarm();
    }
    result
}

#[cfg(target_os = "windows")]
fn send_paste_shortcut() -> Result<()> {
    let mut enigo = Enigo::new(&Settings::default()).context("failed to initialize enigo")?;
    enigo.key(Key::Control, Press)?;
    enigo.key(Key::Unicode('v'), Click)?;
    enigo.key(Key::Control, Release)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn send_paste_shortcut() -> Result<()> {
    let mut enigo = Enigo::new(&Settings::default()).context("failed to initialize enigo")?;
    enigo.key(Key::Meta, Press)?;
    enigo.key(Key::Unicode('v'), Click)?;
    enigo.key(Key::Meta, Release)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::MIN_EMIT_INTERVAL_MS;

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    use super::TextOutput;

    #[test]
    fn min_interval_is_150ms() {
        assert_eq!(MIN_EMIT_INTERVAL_MS, 150);
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[test]
    fn text_output_holds_clipboard() {
        #[cfg(target_os = "linux")]
        fn assert_clipboard_field(output: &TextOutput) {
            let _: &Option<arboard::Clipboard> = &output.clipboard;
        }

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        fn assert_clipboard_field(output: &TextOutput) {
            let _: &arboard::Clipboard = &output.clipboard;
        }

        let _ = assert_clipboard_field;
    }
}
