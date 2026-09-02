use std::fmt;

fn default_restore_primary_selection() -> bool {
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayPosition {
    Bottom,
    Top,
    Center,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayTransparency {
    Auto,
    On,
    Off,
}

/// 貼り付けに使うキー。
///
/// `auto` は宛先の判定を行わず、常に `Shift+Insert` を使う。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PasteShortcutSetting {
    #[default]
    Auto,
    /// 常に `Ctrl+V` を使う手動指定。
    #[serde(alias = "ctrl_v")]
    CtrlV,
    /// 常に `Ctrl+Shift+V` を使う手動指定。
    #[serde(alias = "ctrl_shift_v")]
    CtrlShiftV,
    /// 常に `Shift+Insert` を使う手動指定。
    #[serde(alias = "shift_insert")]
    ShiftInsert,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 発話終了を誰が判定するか。`client`（既定）、`server`、`both`。
    ///
    /// `client` は端末の Silero VAD が無音を検知した時点で `finalize` を送る。
    /// `server` はサーバーの `<end>` を待つ。
    /// `both` はサーバーの終話を使いつつ、端末の無音でも未確定区間を閉じる。
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
    /// 発話中のまま維持する Silero の発話確率しきい値。
    ///
    /// 0.35 は、実機で閾値付近の 0.44〜0.58 が振動していた一方、実際の無音では
    /// 0.26〜0.33 まで下がっていたため、その間に置く。
    pub vad_release_threshold: f32,
    /// マイク入力に掛ける固定倍率。
    pub input_gain: f32,
    /// 開始と判定するのに必要な連続発話時間。
    pub vad_min_speech_ms: u32,
    /// 発話終了とみなす連続無音時間。次の発話を検知するための再武装にのみ使う。
    pub vad_min_silence_ms: u32,
    /// 検知前に遡って送る音声の長さ。
    pub preroll_ms: u32,
    /// 最後の発話区切り通知からこれ以上経過したら接続を閉じる秒数。
    pub idle_close_sec: u32,
    /// マイクデバイス ID。空なら既定。
    pub microphone: String,
    /// 貼り付けまで行うか。
    pub auto_paste: bool,
    /// 発話区切り（`<end>`）ごとに貼り付けるか。false なら停止時に一括。
    pub paste_per_endpoint: bool,
    /// 貼り付けキー。`auto` は常に `Shift+Insert`、それ以外は手動指定。
    #[serde(default)]
    pub paste_shortcut: PasteShortcutSetting,
    /// 貼り付け後に PRIMARY を元の内容へ戻すか。
    #[serde(default = "default_restore_primary_selection")]
    pub restore_primary_selection: bool,
    /// 確定テキストをオーバーレイに保持する時間（ミリ秒）。0 なら即時非表示。
    pub commit_hold_ms: u32,
    /// スプラッシュを表示する時間（ミリ秒）。
    pub splash_ms: u32,
    /// 入力バーの位置。`center`（既定）、`bottom`、`top`。
    pub overlay_position: String,
    /// 入力バーの透過。`auto`（既定）、`on`、`off`。
    pub overlay_transparent: String,
    /// 輪・キャレット・待機 EQ のアニメーションを止めるか。
    pub reduce_motion: bool,
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
            .field("vad_release_threshold", &self.vad_release_threshold)
            .field("input_gain", &self.input_gain)
            .field("vad_min_speech_ms", &self.vad_min_speech_ms)
            .field("vad_min_silence_ms", &self.vad_min_silence_ms)
            .field("preroll_ms", &self.preroll_ms)
            .field("idle_close_sec", &self.idle_close_sec)
            .field("microphone", &self.microphone)
            .field("auto_paste", &self.auto_paste)
            .field("paste_per_endpoint", &self.paste_per_endpoint)
            .field("paste_shortcut", &self.paste_shortcut)
            .field("restore_primary_selection", &self.restore_primary_selection)
            .field("commit_hold_ms", &self.commit_hold_ms)
            .field("splash_ms", &self.splash_ms)
            .field("overlay_position", &self.overlay_position)
            .field("overlay_transparent", &self.overlay_transparent)
            .field("reduce_motion", &self.reduce_motion)
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
            // 発話から出る閾値。入る閾値(0.5)と同じ値、すなわちヒステリシス無しが既定。
            //
            // 2026-08-25 に一度 0.35 にしたが、それは誤りだった。当時のバタつき
            // (9 秒の発話が 5 つに刻まれた)の原因は、server モードで無音の必要時間が
            // 100 ms(4 フレーム)に切り下げられていたことで、閾値の往復ではない。
            // 無音を 300 ms(10 フレーム)に戻した後は、フレーム数の要求だけで
            // 十分に抑制できる。0.35 にすると、発話の末尾で確率が 0.40〜0.48 を
            // うろつく区間を「まだ喋っている」と読み続け、終話が 2.7 秒遅れた。
            // 本当の無音では 0.24〜0.33 まで落ちる。
            vad_release_threshold: 0.50,
            input_gain: 1.0,
            vad_min_speech_ms: 200,
            // 2026-08-25 の実測で決めた。録音した実音声 4 セッション(169 秒)に
            // 端末と同じ Silero VAD を通し、出る閾値 0.30〜0.50 × 無音 200〜500 ms の
            // 20 通りで SpeechGate を再現した結果、分割ゼロかつ終話が最速だったのが
            // 出る閾値 0.50 / 無音 400 ms(実効 416 ms、13 フレーム)である。
            // そのときの「発話終端 → 終話」は中央値 464 ms、最大 768 ms。
            // 300 ms では 26 候補中 3 件が 2 分割された。
            vad_min_silence_ms: 400,
            preroll_ms: 500,
            // 発話間で接続を再利用し、短い間隔の入力ごとに推論サービスを
            // 再起動させない。サーバー側の未応答中は別途、時間に関係なく保持する。
            idle_close_sec: 120,
            microphone: String::new(),
            auto_paste: true,
            paste_per_endpoint: true,
            paste_shortcut: PasteShortcutSetting::Auto,
            restore_primary_selection: true,
            commit_hold_ms: 900,
            splash_ms: 2500,
            overlay_position: "center".to_string(),
            overlay_transparent: "auto".to_string(),
            reduce_motion: false,
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

    pub fn overlay_position(&self) -> OverlayPosition {
        match self.overlay_position.as_str() {
            "top" => OverlayPosition::Top,
            "center" => OverlayPosition::Center,
            _ => OverlayPosition::Center,
        }
    }

    pub fn overlay_transparency(&self) -> OverlayTransparency {
        match self.overlay_transparent.as_str() {
            "on" => OverlayTransparency::On,
            "off" => OverlayTransparency::Off,
            _ => OverlayTransparency::Auto,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PasteShortcutSetting, Settings};
    use std::sync::{Mutex, PoisonError};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        assert_eq!(settings.idle_close_sec, 120);
        assert!(settings.auto_paste);
        assert!(settings.paste_per_endpoint);
        assert_eq!(settings.paste_shortcut, PasteShortcutSetting::Auto);
        assert!(settings.restore_primary_selection);
        assert_eq!(settings.commit_hold_ms, 900);
        assert_eq!(settings.splash_ms, 2500);
        assert_eq!(settings.overlay_position, "center");
        assert_eq!(settings.overlay_transparent, "auto");
        assert!(!settings.reduce_motion);
    }

    #[test]
    fn commit_hold_defaults_to_900_ms() {
        assert_eq!(Settings::default().commit_hold_ms, 900);
    }

    #[test]
    fn vad_release_threshold_defaults_to_0_50_and_can_be_configured() {
        assert_eq!(Settings::default().vad_release_threshold, 0.50);

        let settings: Settings = serde_json::from_str(r#"{"vad_release_threshold":0.4}"#)
            .expect("settings should deserialize");
        assert_eq!(settings.vad_release_threshold, 0.4);
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
        assert_eq!(settings.paste_shortcut, PasteShortcutSetting::Auto);
        assert!(settings.restore_primary_selection);
        assert_eq!(settings.commit_hold_ms, 900);
        assert_eq!(settings.splash_ms, 2500);
        assert_eq!(settings.overlay_position, "center");
        assert_eq!(settings.overlay_transparent, "auto");
        assert!(!settings.reduce_motion);
    }

    #[test]
    fn debug_does_not_contain_removed_api_key() {
        let settings = Settings::default();
        let debug = format!("{settings:?}");
        assert!(!debug.contains("api_key"));
    }

    #[test]
    fn server_url_environment_overrides_settings() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
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
        let _guard = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
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

    #[test]
    fn unknown_overlay_values_fall_back_to_safe_defaults() {
        let settings: Settings = serde_json::from_str(
            r#"{"overlay_position":"diagonal","overlay_transparent":"sometimes"}"#,
        )
        .expect("unknown overlay values should deserialize");
        assert_eq!(settings.overlay_position(), super::OverlayPosition::Center);
        assert_eq!(
            settings.overlay_transparency(),
            super::OverlayTransparency::Auto
        );
    }

    #[test]
    fn overlay_settings_round_trip() {
        let settings: Settings = serde_json::from_str(
            r#"{"overlay_position":"top","overlay_transparent":"off","reduce_motion":true}"#,
        )
        .expect("overlay settings should deserialize");
        let saved = serde_json::to_value(&settings).expect("settings should serialize");
        assert_eq!(saved["overlay_position"], "top");
        assert_eq!(saved["overlay_transparent"], "off");
        assert_eq!(saved["reduce_motion"], true);
    }

    #[test]
    fn paste_shortcut_values_round_trip_and_read_legacy_names() {
        let cases = [
            ("auto", PasteShortcutSetting::Auto),
            ("ctrl-v", PasteShortcutSetting::CtrlV),
            ("ctrl-shift-v", PasteShortcutSetting::CtrlShiftV),
            ("shift-insert", PasteShortcutSetting::ShiftInsert),
        ];
        for (serialized, expected) in cases {
            let settings: Settings =
                serde_json::from_str(&format!(r#"{{"paste_shortcut":"{serialized}"}}"#))
                    .expect("paste shortcut should deserialize");
            assert_eq!(settings.paste_shortcut, expected);
            let saved = serde_json::to_value(&settings).expect("settings should serialize");
            assert_eq!(saved["paste_shortcut"], serialized);
        }

        for (serialized, expected) in [
            ("ctrl_v", PasteShortcutSetting::CtrlV),
            ("ctrl_shift_v", PasteShortcutSetting::CtrlShiftV),
            ("shift_insert", PasteShortcutSetting::ShiftInsert),
        ] {
            let settings: Settings =
                serde_json::from_str(&format!(r#"{{"paste_shortcut":"{serialized}"}}"#))
                    .expect("legacy paste shortcut should deserialize");
            assert_eq!(settings.paste_shortcut, expected);
        }
    }

    #[test]
    fn removed_terminal_window_classes_are_ignored() {
        let settings: Settings = serde_json::from_str(
            r#"{
                "paste_shortcut":"ctrl-shift-v",
                "terminal_window_classes":["old-terminal"],
                "restore_primary_selection":false
            }"#,
        )
        .expect("removed setting should not break deserialization");
        assert_eq!(settings.paste_shortcut, PasteShortcutSetting::CtrlShiftV);
        assert!(!settings.restore_primary_selection);

        let saved = serde_json::to_value(&settings).expect("settings should serialize");
        assert!(saved.get("terminal_window_classes").is_none());
    }
}
