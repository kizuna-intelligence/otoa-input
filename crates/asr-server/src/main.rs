//! ASR サーバー単体のバイナリ。
//!
//! クライアントに同梱された `otoa-input --serve` と中身は同じ。
//! 別の機械でサーバーだけ動かしたいときに使う。

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    otoa_input_platform::console::use_utf8_output();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if otoa_asr_server::Config::help_requested(&args) {
        print!("{}", otoa_asr_server::USAGE);
        return Ok(());
    }
    otoa_asr_server::run(otoa_asr_server::Config::from_process_args()?).await
}
