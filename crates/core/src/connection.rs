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

/// enrollment を要求した契機。provider は診断・監査に使えるが、結果の扱いは
/// 呼び出し側で共通にする。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnrollReason {
    BeforeConnection,
    Warmup,
}

/// 参照音声の enrollment を試した結果。
///
/// リモート失敗はゲートウェイ側で回復できるため、発話を破棄する理由にはならない。
/// 一方、端末の登録・ログイン不足は利用者の操作を待つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnrollOutcome {
    Ready,
    RetryableRemote(String),
    NeedsUserAction(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub email: Option<String>,
}

/// Resolves a connection without exposing a concrete service to the app.
pub trait ConnectionProvider: Send + Sync + 'static {
    fn endpoint(&self, settings: &Settings) -> anyhow::Result<Endpoint>;

    /// 接続を張らずに接続先だけを調べる。
    ///
    /// 一部の provider は [`Self::endpoint`] で、接続直前に必要な準備を行う。
    /// 起動時の表示用（経路の表示や同梱サーバーの判定）でその準備を発火させない
    /// ために分けている。副作用を持たない provider は既定実装のままでよい。
    fn endpoint_hint(&self, settings: &Settings) -> anyhow::Result<Endpoint> {
        self.endpoint(settings)
    }

    /// このセッションで使われるべき方法の名前。`None` なら確認しない。
    ///
    /// **名前を返した provider は、サーバーがその名前を名乗るまで転写を受け取らない。**
    /// 接続先の指定を知らないサーバーは、その指定を黙って捨てて別の経路で処理できて
    /// しまう。実際に、本人の声だけを選んだ利用者の音声が他社のクラウドへ流れていた。
    /// **黙って別の経路に落ちるくらいなら、繋がらないほうがよい。**
    fn expected_backend(&self, _settings: &Settings) -> Option<String> {
        None
    }

    /// 長時間使っていなかった認識サービスを、発話前に起こせるか。
    ///
    /// 常駐サービスなど、起動処理が不要な provider は `false` のままでよい。
    fn supports_warmup(&self, _settings: &Settings) -> bool {
        false
    }

    /// 認識サービスを起こす。`supports_warmup` が `true` のときだけ呼ばれる。
    fn warmup(&self, _settings: &Settings) -> anyhow::Result<()> {
        Ok(())
    }

    /// enrollment をバックグラウンドで試せる条件がそろっているか。
    ///
    /// 新しい provider は、認証情報と参照音声プロファイルの両方をここで確認する。
    /// 旧 provider は `supports_warmup` の既定実装を引き継げるようにしている。
    fn enrollment_is_eligible(&self, settings: &Settings) -> bool {
        self.supports_warmup(settings)
    }

    /// 接続前・ウォームアップで共通に使う enrollment の唯一の入口。
    ///
    /// `warmup` は既存 provider 互換の既定実装だけに残す。アプリ本体はこのメソッド
    /// 以外から enrollment を起動しない。
    fn ensure_enrolled(&self, settings: &Settings, _reason: EnrollReason) -> EnrollOutcome {
        if !self.enrollment_is_eligible(settings) {
            return EnrollOutcome::Ready;
        }
        match self.warmup(settings) {
            Ok(()) => EnrollOutcome::Ready,
            Err(error) => EnrollOutcome::RetryableRemote(error.to_string()),
        }
    }

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
