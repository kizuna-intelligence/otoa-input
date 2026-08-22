use crate::Settings;
use std::sync::atomic::AtomicBool;

/// A resolved ASR connection used by the application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// A protocol-level API key, when the selected server expects one.
    pub api_key: Option<String>,
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
