//! コンソールの文字コードを揃える。

/// Windows のコンソール出力を UTF-8 にする。
///
/// 既定のコードページは日本語環境では 932 (Shift-JIS) で、UTF-8 の文字列を
/// そのまま出すと化ける。`--help` も警告もログも読めなくなるので、
/// 起動時に一度だけ切り替える。他の OS では何もしない。
pub fn use_utf8_output() {
    #[cfg(target_os = "windows")]
    {
        const UTF8: u32 = 65_001;
        // SAFETY: 引数はコードページ番号だけで、失敗しても戻り値が 0 になる。
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleOutputCP(UTF8);
        }
    }
}
