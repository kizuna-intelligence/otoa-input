//! 同梱サーバーへ繋ぐ既定のバイナリ。
//!
//! 別の接続先を使うバイナリは、`otoa_input_app::run` に自分の
//! `ConnectionProvider` を渡して組み立てる。

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    otoa_input_app::run_self_hosted()
}
