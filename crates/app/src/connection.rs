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
        let url = settings
            .resolved_server_url()
            .unwrap_or_else(|| DEFAULT_SERVER_URL.to_string());
        Ok(endpoint_with_auth_token(url, resolve_auth_token()))
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
fn resolve_auth_token() -> Option<String> {
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

fn endpoint_with_auth_token(url: String, auth_token: Option<String>) -> Endpoint {
    let headers = auth_token
        .as_ref()
        .map(|token| vec![("Authorization".to_string(), format!("Bearer {token}"))])
        .unwrap_or_default();

    Endpoint {
        url,
        headers,
        // Keep the token here so connection errors can redact it. Since the
        // endpoint now has a header, callers deliberately omit it from the
        // protocol config and use Authorization as the source of truth.
        api_key: auth_token,
    }
}

#[cfg(test)]
mod tests {
    use super::endpoint_with_auth_token;
    use otoa_input_protocol::{AsrConfig, AsrEvent, AsrSession};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    const TEST_TOKEN: &str = "local-regression-token";

    #[test]
    fn auth_token_becomes_bearer_header_and_remains_available_for_redaction() {
        let endpoint = endpoint_with_auth_token(
            "ws://127.0.0.1:8770/asr/v1".to_string(),
            Some(TEST_TOKEN.to_string()),
        );

        assert_eq!(
            endpoint.headers,
            vec![("Authorization".to_string(), format!("Bearer {TEST_TOKEN}"))]
        );
        assert_eq!(endpoint.api_key.as_deref(), Some(TEST_TOKEN));

        // Gateway-style endpoints with handshake headers must not copy their
        // credential into the protocol config JSON.
        let config_key = endpoint
            .headers
            .is_empty()
            .then(|| endpoint.api_key.clone())
            .flatten();
        assert_eq!(config_key, None);
    }

    #[test]
    fn missing_auth_token_keeps_the_unauthenticated_handshake() {
        let endpoint = endpoint_with_auth_token("ws://127.0.0.1:8770/asr/v1".to_string(), None);

        assert!(endpoint.headers.is_empty());
        assert_eq!(endpoint.api_key, None);
    }

    #[test]
    fn bearer_header_is_sent_in_the_websocket_upgrade_request() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("test listener should have an address");
        let (request_sender, request_receiver) = crossbeam_channel::bounded(1);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("client should connect");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("read timeout should be accepted");

            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream
                    .read(&mut buffer)
                    .expect("request should be readable");
                assert!(count > 0, "client closed before completing the handshake");
                request.extend_from_slice(&buffer[..count]);
            }
            request_sender
                .send(request)
                .expect("request should be observable by the test");
            stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("test response should be writable");
        });

        let endpoint = endpoint_with_auth_token(
            format!("ws://{address}/asr/v1"),
            Some(TEST_TOKEN.to_string()),
        );
        let (_command_sender, commands) = crossbeam_channel::unbounded();
        let (events, event_receiver) = crossbeam_channel::unbounded();
        let client = AsrSession::spawn(
            endpoint.url,
            AsrConfig::realtime_pcm16k(None),
            endpoint.headers,
            commands,
            events,
        );

        let request = request_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("websocket upgrade request should arrive");
        client
            .join()
            .expect("client thread should stop after HTTP 401");
        server.join().expect("test server should stop");
        assert!(matches!(
            event_receiver.recv_timeout(Duration::from_secs(1)),
            Ok(AsrEvent::Failed(_))
        ));

        let request = String::from_utf8(request).expect("HTTP request should be UTF-8");
        let authorization_values = request.lines().filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim())
        });
        assert_eq!(
            authorization_values.collect::<Vec<_>>(),
            vec![format!("Bearer {TEST_TOKEN}")]
        );
    }
}
