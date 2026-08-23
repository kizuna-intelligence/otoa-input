use crate::Settings;
use std::fmt;
use std::sync::atomic::AtomicBool;

/// A resolved ASR connection used by the application.
#[derive(Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// A protocol-level API key, when the selected server expects one.
    pub api_key: Option<String>,
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers = self
            .headers
            .iter()
            .map(|(name, _)| (name.as_str(), "***"))
            .collect::<Vec<_>>();
        formatter
            .debug_struct("Endpoint")
            .field("url", &self.url)
            .field("headers", &headers)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    NeedsSetup { message: String },
    NeedsLogin { message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrepareAction {
    Login,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub email: Option<String>,
}

/// Resolves a connection without exposing a concrete service to the app.
pub trait ConnectionProvider: Send + Sync + 'static {
    fn endpoint(&self, settings: &Settings) -> anyhow::Result<Endpoint>;
    fn readiness(&self) -> Readiness;
    fn prepare(&self) -> Option<PrepareAction>;
    fn authenticate(&self, cancelled: &AtomicBool) -> anyhow::Result<()>;
    fn logout(&self) -> anyhow::Result<()>;
    fn account(&self) -> Option<Account>;
    fn update_settings(&self, settings: &Settings, product_settings: Option<&serde_json::Value>);
}

#[cfg(test)]
mod tests {
    use super::Endpoint;

    #[test]
    fn endpoint_debug_redacts_header_values_and_api_key() {
        let endpoint = Endpoint {
            url: "wss://asr.example/asr/v1".to_string(),
            headers: vec![
                (
                    "Authorization".to_string(),
                    "Bearer handshake-secret".to_string(),
                ),
                ("X-Api-Key".to_string(), "header-secret".to_string()),
            ],
            api_key: Some("config-secret".to_string()),
        };

        let debug = format!("{endpoint:?}");

        assert!(debug.contains("Authorization"));
        assert!(debug.contains("X-Api-Key"));
        assert!(debug.contains("***"));
        assert!(!debug.contains("handshake-secret"));
        assert!(!debug.contains("header-secret"));
        assert!(!debug.contains("config-secret"));
    }
}
