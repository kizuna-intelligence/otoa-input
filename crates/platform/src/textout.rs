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
const PRIMARY_RESTORE_DELAY_MS: u64 = 150;

#[cfg(target_os = "linux")]
const XCLIP_MAX_RETRIES: usize = 3;

/// 貼り付けに使う具体的なキー。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PasteShortcut {
    CtrlV,
    CtrlShiftV,
    #[default]
    ShiftInsert,
}

#[derive(Debug)]
pub enum PasteMethod {
    /// クリップボードに置くだけ。ユーザーが自分で貼る。
    ClipboardOnly,
    /// クリップボードに置き、貼り付けキーを送る。
    ClipboardAndPaste,
}

pub struct TextOutput {
    last_emit: Option<Instant>,
    #[cfg(target_os = "linux")]
    clipboard: Option<arboard::Clipboard>,
    #[cfg(target_os = "linux")]
    paste_shortcut: PasteShortcut,
    #[cfg(target_os = "linux")]
    restore_primary_selection: bool,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    clipboard: arboard::Clipboard,
}

impl TextOutput {
    pub fn new() -> Result<Self> {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let clipboard = arboard::Clipboard::new().context("failed to initialize clipboard")?;

        #[cfg(target_os = "linux")]
        {
            let xclip_available = command_available("xclip");
            let xsel_available = command_available("xsel");
            let wl_copy_available = command_available("wl-copy");
            let xdotool_available = command_available("xdotool");
            tracing::info!(
                xclip = xclip_available,
                xsel = xsel_available,
                wl_copy = wl_copy_available,
                xdotool = xdotool_available,
                "text output command availability"
            );
        }

        Ok(Self {
            last_emit: None,
            #[cfg(target_os = "linux")]
            clipboard: None,
            #[cfg(target_os = "linux")]
            paste_shortcut: PasteShortcut::ShiftInsert,
            #[cfg(target_os = "linux")]
            restore_primary_selection: true,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            clipboard,
        })
    }

    /// 貼り付けキーを設定する。Linux では `auto` の解決結果もここへ渡す。
    pub fn set_paste_shortcut(&mut self, shortcut: PasteShortcut) {
        #[cfg(target_os = "linux")]
        {
            self.paste_shortcut = shortcut;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = shortcut;
    }

    /// PRIMARY を貼り付け前の内容へ戻すか設定する。
    pub fn set_restore_primary_selection(&mut self, restore: bool) {
        #[cfg(target_os = "linux")]
        {
            self.restore_primary_selection = restore;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = restore;
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
    fn emit_linux(&mut self, text: &str, method: PasteMethod) -> Result<()> {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            self.emit_wayland(text, method)
        } else {
            self.emit_x11(text, method)
        }
    }

    #[cfg(target_os = "linux")]
    fn emit_wayland(&mut self, text: &str, method: PasteMethod) -> Result<()> {
        let previous_primary = read_wayland_primary_selection();
        let clipboard_result = if command_available("wl-copy") {
            write_clipboard_command("wl-copy", &[], text)
                .context("failed to set the Wayland clipboard with wl-copy")
        } else {
            tracing::warn!(
                "clipboard write method=wl-copy ok=false reason=unavailable; using arboard fallback"
            );
            self.set_arboard_fallback(text)
        };
        let clipboard_ok = clipboard_result.is_ok();
        if let Err(error) = &clipboard_result {
            tracing::warn!("clipboard write ok=false error={error:#}");
        } else {
            debug!("clipboard write ok=true");
        }

        if matches!(&method, PasteMethod::ClipboardOnly) {
            log_paste(self.paste_shortcut, clipboard_ok, false, "skipped");
            return clipboard_result;
        }

        let primary_ok = match write_clipboard_command("wl-copy", &["--primary"], text) {
            Ok(()) => {
                debug!("primary write method=wl-copy ok=true");
                true
            }
            Err(error) => {
                tracing::warn!("primary write method=wl-copy ok=false error={error:#}");
                false
            }
        };
        let paste_result = send_wayland_paste_shortcut(self.paste_shortcut);
        let restore_status = if self.restore_primary_selection && primary_ok {
            match previous_primary {
                Some(previous) => {
                    thread::sleep(Duration::from_millis(PRIMARY_RESTORE_DELAY_MS));
                    match write_clipboard_bytes("wl-copy", &["--primary"], &previous) {
                        Ok(()) => "ok",
                        Err(error) => {
                            tracing::warn!("primary restore ok=false error={error:#}");
                            "failed"
                        }
                    }
                }
                None => "skipped",
            }
        } else {
            "skipped"
        };
        log_paste(
            self.paste_shortcut,
            clipboard_ok,
            primary_ok,
            restore_status,
        );
        paste_result
    }

    #[cfg(target_os = "linux")]
    fn emit_x11(&mut self, text: &str, method: PasteMethod) -> Result<()> {
        let previous_primary = read_primary_selection();
        let clipboard_result = self.write_clipboard_x11(text);
        let clipboard_ok = clipboard_result.is_ok();
        if let Err(error) = &clipboard_result {
            tracing::warn!("clipboard write ok=false error={error:#}");
        }

        if matches!(&method, PasteMethod::ClipboardOnly) {
            log_paste(self.paste_shortcut, clipboard_ok, false, "skipped");
            return clipboard_result;
        }

        let primary_result = set_primary_selection(text);
        let primary_ok = primary_result.is_ok();
        if let Err(error) = &primary_result {
            tracing::warn!("primary write ok=false error={error:#}");
        }

        let paste_result = send_paste_shortcut(self.paste_shortcut);
        let restore_status = if self.restore_primary_selection && primary_ok {
            match previous_primary {
                Some(previous) => {
                    thread::sleep(Duration::from_millis(PRIMARY_RESTORE_DELAY_MS));
                    match restore_primary_selection(&previous) {
                        Ok(()) => "ok",
                        Err(error) => {
                            tracing::warn!("primary restore ok=false error={error:#}");
                            "failed"
                        }
                    }
                }
                None => "skipped",
            }
        } else {
            "skipped"
        };
        log_paste(
            self.paste_shortcut,
            clipboard_ok,
            primary_ok,
            restore_status,
        );
        paste_result
    }

    #[cfg(target_os = "linux")]
    fn write_clipboard_x11(&mut self, text: &str) -> Result<()> {
        if command_available("xclip") {
            anyhow::ensure!(
                set_and_verify_xclip(text),
                "xclip clipboard verification failed"
            );
            return Ok(());
        }
        if command_available("xsel") {
            write_clipboard_command("xsel", &["--clipboard", "--input"], text)
                .context("failed to set the X11 clipboard with xsel")?;
            debug!("clipboard write method=xsel ok=true");
            return Ok(());
        }
        tracing::warn!(
            "clipboard write method=arboard reason=xclip-and-xsel-unavailable; using arboard clipboard fallback on X11"
        );
        self.set_arboard_fallback(text)
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
}

#[cfg(target_os = "linux")]
fn log_paste(shortcut: PasteShortcut, clipboard_ok: bool, primary_ok: bool, restore_status: &str) {
    tracing::info!(
        "paste: shortcut={} clipboard={} primary={} restore={}",
        paste_shortcut_name(shortcut),
        if clipboard_ok { "ok" } else { "failed" },
        if primary_ok { "ok" } else { "failed" },
        restore_status,
    );
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
    write_clipboard_bytes(command, args, text.as_bytes())
}

#[cfg(target_os = "linux")]
fn write_clipboard_bytes(command: &str, args: &[&str], bytes: &[u8]) -> Result<()> {
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
        .write_all(bytes)
        .with_context(|| format!("failed to write text to {command} stdin"))?;
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
    }
    tracing::warn!(
        "clipboard readback method=xclip result={} retries={}",
        final_result,
        XCLIP_MAX_RETRIES
    );
    false
}

#[cfg(target_os = "linux")]
fn read_primary_selection() -> Option<Vec<u8>> {
    if command_available("xclip") {
        match read_clipboard_command("xclip", &["-o", "-selection", "primary"]) {
            Ok(value) if !value.is_empty() => return Some(value),
            Ok(_) => return None,
            Err(error) => tracing::debug!("primary read method=xclip failed error={error:#}"),
        }
    }
    if command_available("xsel") {
        match read_clipboard_command("xsel", &["--primary", "--output"]) {
            Ok(value) if !value.is_empty() => return Some(value),
            Ok(_) => return None,
            Err(error) => tracing::debug!("primary read method=xsel failed error={error:#}"),
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn set_primary_selection(text: &str) -> Result<()> {
    if command_available("xclip") {
        write_clipboard_command("xclip", &["-selection", "primary"], text)
            .context("failed to set the PRIMARY selection with xclip")?;
        debug!("primary write method=xclip ok=true");
        return Ok(());
    }
    if command_available("xsel") {
        write_clipboard_command("xsel", &["--primary", "--input"], text)
            .context("failed to set the PRIMARY selection with xsel")?;
        debug!("primary write method=xsel ok=true");
        return Ok(());
    }
    anyhow::bail!("cannot set the PRIMARY selection: neither xclip nor xsel is available")
}

#[cfg(target_os = "linux")]
fn restore_primary_selection(previous: &[u8]) -> Result<()> {
    if command_available("xclip") {
        write_clipboard_bytes("xclip", &["-selection", "primary"], previous)
            .context("failed to restore the PRIMARY selection with xclip")?;
        return Ok(());
    }
    if command_available("xsel") {
        write_clipboard_bytes("xsel", &["--primary", "--input"], previous)
            .context("failed to restore the PRIMARY selection with xsel")?;
        return Ok(());
    }
    anyhow::bail!("cannot restore the PRIMARY selection: neither xclip nor xsel is available")
}

#[cfg(target_os = "linux")]
fn read_wayland_primary_selection() -> Option<Vec<u8>> {
    if !command_available("wl-paste") {
        return None;
    }
    match read_clipboard_command("wl-paste", &["--primary"]) {
        Ok(value) if !value.is_empty() => Some(value),
        Ok(_) => None,
        Err(error) => {
            tracing::debug!("primary read method=wl-paste failed error={error:#}");
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn paste_shortcut_name(shortcut: PasteShortcut) -> &'static str {
    match shortcut {
        PasteShortcut::CtrlV => "ctrl-v",
        PasteShortcut::CtrlShiftV => "ctrl-shift-v",
        PasteShortcut::ShiftInsert => "shift-insert",
    }
}

#[cfg(target_os = "linux")]
fn wtype_paste_args(shortcut: PasteShortcut) -> &'static [&'static str] {
    match shortcut {
        PasteShortcut::CtrlV => &["-M", "ctrl", "v", "-m", "ctrl"],
        PasteShortcut::CtrlShiftV => &[
            "-M", "ctrl", "-M", "shift", "v", "-m", "shift", "-m", "ctrl",
        ],
        PasteShortcut::ShiftInsert => &["-M", "shift", "-k", "Insert", "-m", "shift"],
    }
}

#[cfg(target_os = "linux")]
fn send_wayland_paste_shortcut(shortcut: PasteShortcut) -> Result<()> {
    let args = wtype_paste_args(shortcut);
    let command = format!("wtype {}", args.join(" "));
    debug!("paste key command={command} start");
    match Command::new("wtype").args(args).status() {
        Ok(status) if status.success() => {
            debug!("paste key command={command} ok=true");
        }
        Ok(status) => {
            tracing::warn!("paste key command={command} ok=false status={status}");
        }
        Err(error) => {
            tracing::warn!("paste key command={command} ok=false error={error}");
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
struct ModifierKeyupGuard {
    modifiers: &'static [&'static str],
    armed: bool,
}

#[cfg(target_os = "linux")]
impl ModifierKeyupGuard {
    fn new(modifiers: &'static [&'static str]) -> Self {
        Self {
            modifiers,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(target_os = "linux")]
impl Drop for ModifierKeyupGuard {
    fn drop(&mut self) {
        if !self.armed || self.modifiers.is_empty() {
            return;
        }
        let step = format!("keyup {} (cleanup)", self.modifiers.join(" "));
        if let Err(error) = run_xdotool_key_step(&step, "keyup", self.modifiers) {
            tracing::error!("paste key step={} ok=false error={error:#}", step);
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
        Ok(status) if status.success() => {
            debug!("paste key step={step} command={command} ok=true");
            Ok(())
        }
        Ok(status) => {
            tracing::warn!("paste key step={step} command={command} ok=false status={status}");
            anyhow::bail!("paste key step {step} failed: {command}");
        }
        Err(error) => Err(error)
            .with_context(|| format!("failed to execute paste key step {step}: {command}")),
    }
}

#[cfg(target_os = "linux")]
fn paste_shortcut_args(shortcut: PasteShortcut) -> &'static [&'static str] {
    match shortcut {
        PasteShortcut::CtrlV => &[
            "keyup", "ctrl", "shift", "alt", "super", "keydown", "ctrl", "keydown", "v", "keyup",
            "v", "keyup", "ctrl",
        ],
        PasteShortcut::CtrlShiftV => &[
            "keyup", "ctrl", "shift", "alt", "super", "keydown", "ctrl", "keydown", "shift",
            "keydown", "v", "keyup", "v", "keyup", "shift", "keyup", "ctrl",
        ],
        PasteShortcut::ShiftInsert => &[
            "keyup", "ctrl", "shift", "alt", "super", "keydown", "shift", "key", "Insert", "keyup",
            "shift",
        ],
    }
}

#[cfg(target_os = "linux")]
fn modifier_keyup_guard_modifiers(shortcut: PasteShortcut) -> &'static [&'static str] {
    match shortcut {
        PasteShortcut::CtrlV => &["ctrl"],
        PasteShortcut::CtrlShiftV => &["ctrl", "shift"],
        PasteShortcut::ShiftInsert => &["shift"],
    }
}

#[cfg(target_os = "linux")]
fn send_paste_shortcut(shortcut: PasteShortcut) -> Result<()> {
    let mut modifier_keyup_guard =
        ModifierKeyupGuard::new(modifier_keyup_guard_modifiers(shortcut));
    let args = paste_shortcut_args(shortcut);
    let result = run_xdotool_key_step(&format!("xdotool {}", args.join(" ")), args[0], &args[1..]);
    if result.is_ok() {
        modifier_keyup_guard.disarm();
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
    use super::{PasteShortcut, MIN_EMIT_INTERVAL_MS};

    #[test]
    fn min_interval_is_150ms() {
        assert_eq!(MIN_EMIT_INTERVAL_MS, 150);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn default_shortcut_is_shift_insert() {
        assert_eq!(PasteShortcut::default(), PasteShortcut::ShiftInsert);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn xdotool_paste_arguments_use_the_requested_shortcut() {
        assert_eq!(
            super::paste_shortcut_args(PasteShortcut::CtrlV),
            &[
                "keyup", "ctrl", "shift", "alt", "super", "keydown", "ctrl", "keydown", "v",
                "keyup", "v", "keyup", "ctrl"
            ]
        );
        assert_eq!(
            super::paste_shortcut_args(PasteShortcut::CtrlShiftV),
            &[
                "keyup", "ctrl", "shift", "alt", "super", "keydown", "ctrl", "keydown", "shift",
                "keydown", "v", "keyup", "v", "keyup", "shift", "keyup", "ctrl"
            ]
        );
        assert_eq!(
            super::paste_shortcut_args(PasteShortcut::ShiftInsert),
            &[
                "keyup", "ctrl", "shift", "alt", "super", "keydown", "shift", "key", "Insert",
                "keyup", "shift"
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wtype_paste_arguments_use_the_requested_shortcut() {
        assert_eq!(
            super::wtype_paste_args(PasteShortcut::CtrlV),
            &["-M", "ctrl", "v", "-m", "ctrl"]
        );
        assert_eq!(
            super::wtype_paste_args(PasteShortcut::CtrlShiftV),
            &["-M", "ctrl", "-M", "shift", "v", "-m", "shift", "-m", "ctrl"]
        );
        assert_eq!(
            super::wtype_paste_args(PasteShortcut::ShiftInsert),
            &["-M", "shift", "-k", "Insert", "-m", "shift"]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shortcut_names_are_stable_for_logs() {
        assert_eq!(super::paste_shortcut_name(PasteShortcut::CtrlV), "ctrl-v");
        assert_eq!(
            super::paste_shortcut_name(PasteShortcut::CtrlShiftV),
            "ctrl-shift-v"
        );
        assert_eq!(
            super::paste_shortcut_name(PasteShortcut::ShiftInsert),
            "shift-insert"
        );
    }
}
