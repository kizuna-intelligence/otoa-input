/// OSS 版の起動処理。
///
/// GUI とコンソールは Windows の実行形式だけを分け、中身は必ずここを通す。
pub fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    otoa_input_app::run_self_hosted()
}
