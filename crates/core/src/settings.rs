use std::fmt;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 発話終了を誰が判定するか。`client`（既定）と `server`。
    ///
    /// `client` は端末の Silero VAD が無音を検知した時点で `finalize` を送る。
    /// `server` はサーバーの `<end>` を待つ。
    ///
    /// 既定が `client` なのは、待受を止めないために端末が必ず VAD を持ち、
    /// 発話の開始も終了も既に知っているからである。判定をサーバーにも持たせると
    /// 二重になり、片方が「発話は終わった」と見なした瞬間に噛み合わなくなる。
    ///
    /// 同梱の OSS サーバーは終話を判定しないので `client` 専用である。
    /// `server` は Otoa クラウドのように `<end>` を返す接続先でだけ使う。
    pub endpoint_mode: String,
    /// 接続先の Otoa ASR Protocol サーバー。
    ///
    /// **空にしておくと、接続先の実装が持つ既定値を使う。** 既定値をここに
    /// 置かないのは、何が既定かは接続先の実装だけが知っているからである。
    #[serde(alias = "gateway_url")] // 後方互換: 旧キー名
    pub server_url: String,
    /// 同梱サーバーで使う認識エンジン。`reazonspeech`（既定）と `kodama`。
    ///
    /// 別の機械のサーバーや、エンジンを自分で決める接続先に繋ぐ構成では
    /// 意味を持たない。同梱サーバーを自分で起動するときだけ効く。
    pub asr_engine: String,
    /// 言語ヒント。空なら自動検出。
    pub language_hints: Vec<String>,
    /// 起動時に待受を始めるか。
    pub listening_enabled: bool,
    /// 空なら同梱モデルを探す。
    pub vad_model_path: String,
    /// Silero の発話確率しきい値。
    pub vad_threshold: f32,
    /// マイク入力に掛ける固定倍率。
    pub input_gain: f32,
    /// 開始と判定するのに必要な連続発話時間。
    pub vad_min_speech_ms: u32,
    /// 発話終了とみなす連続無音時間。次の発話を検知するための再武装にのみ使う。
    pub vad_min_silence_ms: u32,
    /// 検知前に遡って送る音声の長さ。
    pub preroll_ms: u32,
    /// ASR サーバー側の発話区切り判定を遅延させる上限。
    pub endpoint_max_delay_ms: u32,
    /// ASR サーバー側の発話区切り判定の感度。
    pub endpoint_sensitivity: f32,
    /// ASR サーバー側の発話区切り判定の遅延調整レベル。
    pub endpoint_latency_level: u8,
    /// 最後の発話区切り通知からこれ以上経過したら接続を閉じる秒数。
    pub idle_close_sec: u32,
    /// マイクデバイス ID。空なら既定。
    pub microphone: String,
    /// 貼り付けまで行うか。
    pub auto_paste: bool,
    /// 発話区切り（`<end>`）ごとに貼り付けるか。false なら停止時に一括。
    pub paste_per_endpoint: bool,
    /// 確定テキストをオーバーレイに保持する時間（ミリ秒）。0 なら即時非表示。
    pub commit_hold_ms: u32,
    /// スプラッシュを表示する時間（ミリ秒）。
    pub splash_ms: u32,
}

impl fmt::Debug for Settings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Settings")
            .field("endpoint_mode", &self.endpoint_mode)
            .field("server_url", &self.server_url)
            .field("asr_engine", &self.asr_engine)
            .field("language_hints", &self.language_hints)
            .field("listening_enabled", &self.listening_enabled)
            .field("vad_model_path", &self.vad_model_path)
            .field("vad_threshold", &self.vad_threshold)
            .field("input_gain", &self.input_gain)
            .field("vad_min_speech_ms", &self.vad_min_speech_ms)
            .field("vad_min_silence_ms", &self.vad_min_silence_ms)
            .field("preroll_ms", &self.preroll_ms)
            .field("endpoint_max_delay_ms", &self.endpoint_max_delay_ms)
            .field("endpoint_sensitivity", &self.endpoint_sensitivity)
            .field("endpoint_latency_level", &self.endpoint_latency_level)
            .field("idle_close_sec", &self.idle_close_sec)
            .field("microphone", &self.microphone)
            .field("auto_paste", &self.auto_paste)
            .field("paste_per_endpoint", &self.paste_per_endpoint)
            .field("commit_hold_ms", &self.commit_hold_ms)
            .field("splash_ms", &self.splash_ms)
            .finish()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            endpoint_mode: "client".to_string(),
            server_url: String::new(),
            asr_engine: "reazonspeech".to_string(),
            language_hints: vec!["ja".to_string()],
            listening_enabled: true,
            vad_model_path: String::new(),
            vad_threshold: 0.5,
            input_gain: 1.0,
            vad_min_speech_ms: 200,
            vad_min_silence_ms: 300,
            preroll_ms: 500,
            endpoint_max_delay_ms: 1500,
            endpoint_sensitivity: 0.0,
            endpoint_latency_level: 0,
            idle_close_sec: 15,
            microphone: String::new(),
            auto_paste: true,
            paste_per_endpoint: true,
            commit_hold_ms: 0,
            splash_ms: 2500,
        }
    }
}

impl Settings {
    /// 環境変数、設定の順に接続先を解決する。
    ///
    /// どちらも空なら `None` を返す。既定値の決定は接続先の実装に委ねる。
    pub fn resolved_server_url(&self) -> Option<String> {
        std::env::var("OTOA_SERVER_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| (!self.server_url.trim().is_empty()).then(|| self.server_url.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn defaults_are_upstream_jp() {
        let settings = Settings::default();
        assert_eq!(settings.language_hints, vec!["ja"]);
        assert!(settings.listening_enabled);
        assert_eq!(settings.asr_engine, "reazonspeech");
        assert_eq!(settings.vad_threshold, 0.5);
        assert_eq!(settings.input_gain, 1.0);
        assert_eq!(settings.vad_min_speech_ms, 200);
        assert_eq!(settings.preroll_ms, 500);
        assert_eq!(settings.idle_close_sec, 15);
        assert!(settings.auto_paste);
        assert!(settings.paste_per_endpoint);
        assert_eq!(settings.commit_hold_ms, 0);
        assert_eq!(settings.splash_ms, 2500);
    }

    #[test]
    fn default_endpoint_tuning_favors_complete_utterances() {
        let settings = Settings::default();
        assert_eq!(settings.endpoint_max_delay_ms, 1500);
        assert_eq!(settings.endpoint_sensitivity, 0.0);
        assert_eq!(settings.endpoint_latency_level, 0);
    }

    #[test]
    fn commit_hold_defaults_to_zero_ms() {
        assert_eq!(Settings::default().commit_hold_ms, 0);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let settings: Settings =
            serde_json::from_str(r#"{}"#).expect("settings should deserialize");
        assert!(settings.server_url.is_empty());
        assert_eq!(settings.asr_engine, "reazonspeech");
        assert_eq!(settings.language_hints, Settings::default().language_hints);
        assert_eq!(settings.vad_threshold, Settings::default().vad_threshold);
        assert_eq!(settings.input_gain, Settings::default().input_gain);
        assert_eq!(settings.auto_paste, Settings::default().auto_paste);
        assert_eq!(settings.commit_hold_ms, 0);
        assert_eq!(settings.splash_ms, 2500);
    }

    #[test]
    fn debug_does_not_contain_removed_api_key() {
        let settings = Settings::default();
        let debug = format!("{settings:?}");
        assert!(!debug.contains("api_key"));
    }

    #[test]
    fn server_url_environment_overrides_settings() {
        let settings = Settings {
            server_url: "wss://settings.example/ws/asr".to_string(),
            ..Settings::default()
        };
        let previous = std::env::var_os("OTOA_SERVER_URL");
        std::env::set_var("OTOA_SERVER_URL", "wss://environment.example/ws/asr");
        assert_eq!(
            settings.resolved_server_url().as_deref(),
            Some("wss://environment.example/ws/asr")
        );
        match previous {
            Some(value) => std::env::set_var("OTOA_SERVER_URL", value),
            None => std::env::remove_var("OTOA_SERVER_URL"),
        }
    }

    #[test]
    fn unset_server_url_leaves_the_default_to_the_provider() {
        // 既定値を core が持つと、接続先を差し替えたときに間違った先へ繋ぐ。
        assert_eq!(Settings::default().resolved_server_url(), None);
    }

    #[test]
    fn the_old_gateway_url_key_still_loads() {
        let settings: Settings =
            serde_json::from_str(r#"{"gateway_url":"wss://old.example/ws/asr"}"#)
                .expect("settings should deserialize");
        assert_eq!(settings.server_url, "wss://old.example/ws/asr");
    }
}
