use otoa_input_core::ConnectionProvider;
use otoa_input_core::{Account, Endpoint, PrepareAction, Readiness, Settings as CoreSettings};
use otoa_input_platform::load_from_ancestors;
use std::sync::atomic::AtomicBool;

/// 自分で立てた Otoa ASR Protocol サーバーへ直接繋ぐ。
///
/// 設定は接続のたびに渡されるので、この型は何も持たない。
#[derive(Debug, Default)]
pub struct SelfHostedProvider;

/// 同梱の `otoa-asr-server` を既定の設定で起動したときのアドレス。
///
/// 既定値をこちらに置くのは、何が既定かを知っているのが接続先の実装だけだからである。
/// core に置くと、別の接続先を使うビルドが設定なしで自分のローカルへ繋いでしまう。
pub const DEFAULT_SERVER_URL: &str = "ws://127.0.0.1:8770/asr/v1";

impl ConnectionProvider for SelfHostedProvider {
    fn endpoint(&self, settings: &CoreSettings) -> anyhow::Result<Endpoint> {
        Ok(Endpoint {
            url: settings
                .resolved_server_url()
                .unwrap_or_else(|| DEFAULT_SERVER_URL.to_string()),
            headers: Vec::new(),
            api_key: resolve_public_api_key(),
        })
    }

    /// 同梱サーバーの既定値があるので、設定が空でも使える。
    fn readiness(&self) -> Readiness {
        Readiness::Ready
    }

    fn prepare(&self) -> Option<PrepareAction> {
        None
    }

    fn authenticate(&self, _cancelled: &AtomicBool) -> anyhow::Result<()> {
        anyhow::bail!("ログインは OSS 版では使用しません")
    }

    fn logout(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn account(&self) -> Option<Account> {
        None
    }

    fn update_settings(
        &self,
        _settings: &CoreSettings,
        _product_settings: Option<&serde_json::Value>,
    ) {
    }
}

/// サーバーが `--auth-token` を要求する場合に送るトークン。
/// 環境変数、なければ上位ディレクトリの `.env` から読む。
fn resolve_public_api_key() -> Option<String> {
    if let Some(value) = std::env::var("OTOA_ASR_AUTH_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
    {
        return Some(value);
    }
    std::env::current_dir()
        .ok()
        .map(|directory| load_from_ancestors(&directory))
        .and_then(|dotenv| dotenv.get("OTOA_ASR_AUTH_TOKEN").cloned())
        .filter(|value| !value.is_empty())
}
