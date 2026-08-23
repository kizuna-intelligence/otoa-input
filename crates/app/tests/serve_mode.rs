use std::process::{Command, Output};

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_otoa-input"))
        .args(arguments)
        .output()
        .expect("otoa-input should run")
}

#[test]
fn serve_help_uses_the_embedded_server_command_name() {
    let output = run(&["--serve", "--help"]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help should be UTF-8");
    assert!(stdout.contains("otoa-input --serve — Otoa ASR Protocol v1"));
    assert!(stdout.contains("otoa-input --serve --asr-model-dir=<dir>"));
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
