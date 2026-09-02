//! コマンド操作用のバイナリ。
//!
//! Windows では GUI 本体とコンソールの有無を実行時に切り替えられないため、
//! `--help`、接続確認、貼り付け確認、サーバーモードはこちらから使う。

#[path = "self_hosted_main.rs"]
mod self_hosted_main;

fn main() -> anyhow::Result<()> {
    self_hosted_main::run()
}
