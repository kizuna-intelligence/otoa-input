#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

//! 同梱サーバーへ繋ぐ既定のバイナリ。
//!
//! 別の接続先を使うバイナリは、`otoa_input_app::run` に自分の
//! `ConnectionProvider` を渡して組み立てる。

#[path = "self_hosted_main.rs"]
mod self_hosted_main;

fn main() -> anyhow::Result<()> {
    self_hosted_main::run()
}
