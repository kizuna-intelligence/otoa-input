use std::fmt;

#[derive(Clone, Copy, Debug)]
pub struct EndpointTuning {
    pub max_delay_ms: u32,
    pub sensitivity: f32,
    pub latency_level: u8,
}

/// ASR 開始メッセージ。接続後に最初の text frame として送る。
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct AsrConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub model: String,
    pub audio_format: String,
    pub sample_rate: u32,
    pub num_channels: u32,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub language_hints: Vec<String>,

    pub enable_endpoint_detection: bool,

    /// 発話区切りを誰が決めるか。`"client"` と `"server"`。
    ///
    /// `"client"` のとき、サーバーは自前の終話判定を使わず、`finalize` を
    /// 受け取るまで音声を溜め続ける。省略された場合の解釈はサーバー側に委ねる。
    ///
    /// これは Otoa のクライアントとサーバーの間の取り決めであり、上流の
    /// 認識サービスへ渡してはならない。ゲートウェイは転送前に取り除く。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_mode: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_endpoint_delay_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_sensitivity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_latency_adjustment_level: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_reference_id: Option<String>,
}

impl fmt::Debug for AsrConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AsrConfig")
            .field("api_key", &"***")
            .field("model", &self.model)
            .field("audio_format", &self.audio_format)
            .field("sample_rate", &self.sample_rate)
            .field("num_channels", &self.num_channels)
            .field("language_hints", &self.language_hints)
            .field("enable_endpoint_detection", &self.enable_endpoint_detection)
            .field("endpoint_mode", &self.endpoint_mode)
            .field("max_endpoint_delay_ms", &self.max_endpoint_delay_ms)
            .field("endpoint_sensitivity", &self.endpoint_sensitivity)
            .field(
                "endpoint_latency_adjustment_level",
                &self.endpoint_latency_adjustment_level,
            )
            .field("client_reference_id", &self.client_reference_id)
            .finish()
    }
}

impl AsrConfig {
    /// 16 kHz / mono / s16le、endpoint detection 有効、日本語 hint の既定構成。
    pub fn realtime_pcm16k(api_key: Option<String>) -> Self {
        Self {
            api_key,
            model: "stt-rt-v5".to_string(),
            audio_format: "pcm_s16le".to_string(),
            sample_rate: 16_000,
            num_channels: 1,
            language_hints: vec!["ja".to_string()],
            enable_endpoint_detection: true,
            endpoint_mode: None,
            max_endpoint_delay_ms: None,
            endpoint_sensitivity: None,
            endpoint_latency_adjustment_level: None,
            client_reference_id: None,
        }
    }

    /// 発話区切りの決定者を指定する。
    ///
    /// 省略せず必ず送る。省略すると、終話判定を持たないサーバーに対して
    /// `"server"` を指定した設定が黙って通り、クライアントは永久に来ない
    /// `<end>` を待ち続ける。
    pub fn with_endpoint_mode(mut self, mode: &str) -> Self {
        self.endpoint_mode = Some(mode.to_string());
        self
    }

    pub fn with_endpoint_tuning(mut self, tuning: EndpointTuning) -> Self {
        self.max_endpoint_delay_ms = Some(tuning.max_delay_ms);
        self.endpoint_sensitivity = Some(tuning.sensitivity);
        self.endpoint_latency_adjustment_level = Some(tuning.latency_level);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::AsrConfig;

    #[test]
    fn config_debug_masks_api_key() {
        let config = AsrConfig::realtime_pcm16k(Some("sk-secret-value".to_string()));
        let debug = format!("{config:?}");
        assert!(!debug.contains("sk-secret-value"));
        assert!(debug.contains("***"));
    }

    #[test]
    fn endpoint_tuning_is_serialized() {
        let config = AsrConfig::realtime_pcm16k(Some("test-key".to_string())).with_endpoint_tuning(
            super::EndpointTuning {
                max_delay_ms: 1500,
                sensitivity: 0.3,
                latency_level: 2,
            },
        );
        let json = serde_json::to_value(config).expect("config should serialize");
        assert_eq!(json["max_endpoint_delay_ms"], 1500);
        assert_eq!(json["endpoint_sensitivity"], serde_json::json!(0.3_f32));
        assert_eq!(json["endpoint_latency_adjustment_level"], 2);
    }

    #[test]
    fn no_tuning_omits_fields() {
        let config = AsrConfig::realtime_pcm16k(Some("test-key".to_string()));
        let json = serde_json::to_value(config).expect("config should serialize");
        let object = json.as_object().expect("config should be an object");
        assert!(!object.contains_key("max_endpoint_delay_ms"));
        assert!(!object.contains_key("endpoint_sensitivity"));
        assert!(!object.contains_key("endpoint_latency_adjustment_level"));
    }

    #[test]
    fn endpoint_mode_is_always_serialized() {
        // 省略すると、終話判定を持たないサーバーに "server" 設定が黙って通る。
        for mode in ["client", "server"] {
            let json =
                serde_json::to_value(AsrConfig::realtime_pcm16k(None).with_endpoint_mode(mode))
                    .expect("config should serialize");
            assert_eq!(json["endpoint_mode"], mode);
        }

        // 指定しなければ送らない。他実装のサーバー向けの既定である。
        let json = serde_json::to_value(AsrConfig::realtime_pcm16k(None))
            .expect("config should serialize");
        assert!(!json
            .as_object()
            .expect("config should be an object")
            .contains_key("endpoint_mode"));
    }

    #[test]
    fn gateway_config_omits_api_key() {
        let config = AsrConfig::realtime_pcm16k(None);
        let object = serde_json::to_value(config)
            .expect("config should serialize")
            .as_object()
            .cloned()
            .expect("config should be an object");
        assert!(!object.contains_key("api_key"));
    }
}
