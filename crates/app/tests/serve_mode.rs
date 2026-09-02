use std::process::{Command, Output};

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_otoa-input-console"))
        .args(arguments)
        .output()
        .expect("otoa-input-console should run")
}

#[test]
fn console_help_names_the_console_entrypoint_without_renaming_the_settings_directory() {
    let output = run(&["--help"]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help should be UTF-8");
    assert!(stdout.contains("otoa-input-console — 話した内容をカーソル位置へ貼り付ける音声入力"));
    assert!(stdout.contains("otoa-input-console [オプション]"));
    assert!(stdout.contains("~/.config/otoa-input-oss/settings.json"));
    assert!(!stdout.contains("otoa-input-console-oss"));
}

#[test]
fn console_version_names_the_console_entrypoint() {
    let output = run(&["--version"]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("version should be UTF-8");
    assert_eq!(
        stdout.trim(),
        format!("otoa-input-console {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn serve_help_uses_the_embedded_server_command_name() {
    let output = run(&["--serve", "--help"]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help should be UTF-8");
    assert!(stdout.contains("otoa-input-console --serve — Otoa ASR Protocol v1"));
    assert!(stdout.contains("otoa-input-console --serve --asr-model-dir=<dir>"));
    assert!(stdout.contains("--auth-token=<token>"));
    assert!(!stdout.contains("カーソル位置へ貼り付ける音声入力"));
}

#[test]
fn serve_rejects_unknown_server_options_with_a_failure_exit_code() {
    let output = run(&["--serve", "--unknown-server-option"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("error should be UTF-8");
    assert!(stderr.contains("unknown option: --unknown-server-option"));
}

#[test]
fn serve_propagates_server_config_validation_errors() {
    let output = run(&["--serve", "--port=0"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("error should be UTF-8");
    assert!(stderr.contains("port must be between 1 and 65535"));
}
