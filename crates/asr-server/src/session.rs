use crate::{agreement::Agreement, asr::Transcriber, audio::SAMPLE_RATE, config::Config};
use anyhow::{anyhow, Result};
use otoa_input_protocol::{AsrConfig, AsrResponse, AsrToken, TOKEN_END, TOKEN_FIN};
use std::{sync::Arc, time::Instant};
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Connecting,
    Ready,
    Streaming,
    Closing,
    Closed,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("protocol error {code} {error_type}: {message}")]
    Protocol {
        code: u32,
        error_type: &'static str,
        message: String,
    },
    #[error("ASR recognition failed: {0}")]
    Recognition(#[source] anyhow::Error),
}

impl SessionError {
    pub fn protocol(code: u32, error_type: &'static str, message: impl Into<String>) -> Self {
        Self::Protocol {
            code,
            error_type,
            message: message.into(),
        }
    }

    pub fn error_response(&self) -> AsrResponse {
        match self {
            Self::Protocol {
                code,
                error_type,
                message,
            } => error_response(*code, error_type, message),
            Self::Recognition(error) => error_response(500, "asr_failed", &error.to_string()),
        }
    }

    pub fn close_code(&self) -> u16 {
        match self {
            Self::Protocol { .. } => 1008,
            Self::Recognition(_) => 1011,
        }
    }
}

#[derive(Debug, Default)]
pub struct SessionAction {
    pub responses: Vec<AsrResponse>,
    pub close_code: Option<u16>,
}

impl SessionAction {
    fn response(response: AsrResponse) -> Self {
        Self {
            responses: vec![response],
            close_code: None,
        }
    }
}

/// PCM byte framing for arbitrary websocket binary message boundaries.
#[derive(Debug, Default)]
pub struct Pcm16Buffer {
    pending_byte: Option<u8>,
}

impl Pcm16Buffer {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<f32> {
        let mut samples = Vec::with_capacity(bytes.len().div_ceil(2));
        let mut offset = 0;
        if let Some(low) = self.pending_byte.take() {
            if let Some(&high) = bytes.first() {
                samples.push(i16::from_le_bytes([low, high]) as f32 / 32_768.0);
                offset = 1;
            } else {
                self.pending_byte = Some(low);
                return samples;
            }
        }
        let complete_len = (bytes.len() - offset) & !1;
        for pair in bytes[offset..offset + complete_len].chunks_exact(2) {
            samples.push(i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32_768.0);
        }
        if offset + complete_len < bytes.len() {
            self.pending_byte = Some(bytes[offset + complete_len]);
        }
        samples
    }

    pub fn has_pending(&self) -> bool {
        self.pending_byte.is_some()
    }
}

/// Repeated whole-buffer decoding without a pending decode queue.
pub struct PseudoStream {
    decoder: Arc<dyn Transcriber>,
    partial_interval_ms: u32,
    pseudo_stream: bool,
    chunks: Vec<Vec<f32>>,
    audio_samples: usize,
    next_partial_ms: f32,
    partial_task: Option<JoinHandle<Result<String>>>,
    transcribe_count: usize,
}

impl PseudoStream {
    pub fn new(
        decoder: Arc<dyn Transcriber>,
        partial_interval_ms: u32,
        pseudo_stream: bool,
    ) -> Self {
        Self {
            decoder,
            partial_interval_ms,
            pseudo_stream,
            chunks: Vec::new(),
            audio_samples: 0,
            next_partial_ms: partial_interval_ms as f32,
            partial_task: None,
            transcribe_count: 0,
        }
    }

    pub fn audio_ms(&self) -> f32 {
        self.audio_samples as f32 * 1000.0 / SAMPLE_RATE as f32
    }

    pub fn buffer(&self) -> Vec<f32> {
        let mut buffer = Vec::with_capacity(self.audio_samples);
        for chunk in &self.chunks {
            buffer.extend_from_slice(chunk);
        }
        buffer
    }

    pub fn append(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        self.audio_samples += samples.len();
        self.chunks.push(samples.to_vec());
    }

    pub async fn advance(&mut self) -> Result<Option<String>> {
        let completed = self.collect_completed().await?;
        if self.pseudo_stream && self.audio_ms() >= self.next_partial_ms {
            while self.audio_ms() >= self.next_partial_ms {
                self.next_partial_ms += self.partial_interval_ms as f32;
            }
            if self.partial_task.is_none() {
                self.partial_task = Some(self.spawn_decode(self.buffer()));
            }
        }
        tokio::task::yield_now().await;
        let completed_after_schedule = self.collect_completed().await?;
        Ok(completed_after_schedule.or(completed))
    }

    /// 発話が進行中か。バッファに音声があることが唯一の判定基準。
    pub fn has_audio(&self) -> bool {
        !self.chunks.is_empty()
    }

    pub async fn finalize(&mut self) -> Result<String> {
        if let Some(task) = self.partial_task.take() {
            // The partial result is deliberately discarded. The final decode
            // below always uses the newest complete buffer.
            let _ = task.await;
        }
        if self.chunks.is_empty() {
            return Ok(String::new());
        }
        self.decode(self.buffer()).await
    }

    pub async fn cancel(&mut self) {
        if let Some(task) = self.partial_task.take() {
            task.abort();
            let _ = task.await;
        }
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.audio_samples = 0;
        self.next_partial_ms = self.partial_interval_ms as f32;
    }

    async fn collect_completed(&mut self) -> Result<Option<String>> {
        let Some(task) = self.partial_task.as_ref() else {
            return Ok(None);
        };
        if !task.is_finished() {
            return Ok(None);
        }
        let task = self
            .partial_task
            .take()
            .expect("partial task exists after is_finished check");
        let result = task
            .await
            .map_err(|error| anyhow!("partial transcription task failed: {error}"))??;
        Ok(Some(result))
    }

    fn spawn_decode(&mut self, snapshot: Vec<f32>) -> JoinHandle<Result<String>> {
        self.transcribe_count += 1;
        let count = self.transcribe_count;
        let decoder = Arc::clone(&self.decoder);
        let started = Instant::now();
        tokio::task::spawn_blocking(move || {
            let result = decoder.transcribe(&snapshot);
            let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
            match &result {
                Ok(_) => tracing::info!(count, duration_ms, "transcribe completed"),
                Err(error) => {
                    tracing::error!(count, duration_ms, error = %error, "transcribe failed")
                }
            }
            result
        })
    }

    async fn decode(&mut self, snapshot: Vec<f32>) -> Result<String> {
        let task = self.spawn_decode(snapshot);
        task.await
            .map_err(|error| anyhow!("transcription task failed: {error}"))?
    }
}

#[derive(Clone, Copy)]
enum Marker {
    End,
    MaxUtterance,
    Fin,
}

/// 1 本の WebSocket 接続。
///
/// **発話の区切りはクライアントが `finalize` で決める。このサーバーは終話を
/// 判定しない。** 判定を両側に持たせると、片方が「発話は終わった」と見なした
/// 時点で音声の蓄積が止まり、もう片方は空の結果を受け取る。
pub struct Session {
    pub state: SessionState,
    config: Config,
    settings: Option<AsrConfig>,
    pseudo: PseudoStream,
    agreement: Agreement,
    pcm_input: Pcm16Buffer,
    pending_responses: Vec<AsrResponse>,
}

impl Session {
    pub fn new(config: Config, asr: Arc<dyn Transcriber>) -> Self {
        let pseudo = PseudoStream::new(
            Arc::clone(&asr),
            config.partial_interval_ms,
            config.pseudo_stream,
        );
        let agreement = Agreement::new(config.partial_tail_margin_chars);
        Self {
            state: SessionState::Connecting,
            config,
            settings: None,
            pseudo,
            agreement,
            pcm_input: Pcm16Buffer::default(),
            pending_responses: Vec::new(),
        }
    }

    pub async fn handle_text(
        &mut self,
        message: &str,
    ) -> std::result::Result<SessionAction, SessionError> {
        if self.settings.is_none() {
            if message.is_empty() {
                return Err(SessionError::protocol(
                    400,
                    "invalid_config",
                    "最初のフレームは設定 JSON でなければなりません",
                ));
            }
            self.settings = Some(parse_client_settings(message)?);
            self.state = SessionState::Ready;
            tracing::info!("configuration JSON received");
            return Ok(SessionAction::default());
        }

        if message.is_empty() {
            return self.finish_connection().await;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(message) else {
            // Unknown text messages are intentionally ignored by protocol v1.
            return Ok(SessionAction::default());
        };
        if value.get("type").and_then(serde_json::Value::as_str) == Some("finalize") {
            return self.finalize().await;
        }
        // keepalive and all other text controls are no-ops.
        Ok(SessionAction::default())
    }

    pub async fn handle_binary(
        &mut self,
        bytes: &[u8],
    ) -> std::result::Result<SessionAction, SessionError> {
        if self.settings.is_none() {
            return Err(SessionError::protocol(
                400,
                "invalid_config",
                "設定 JSON を先に送信してください",
            ));
        }
        if bytes.is_empty() {
            return self.finish_connection().await;
        }
        if self.state == SessionState::Ready {
            self.state = SessionState::Streaming;
        }
        let samples = self.pcm_input.feed(bytes);
        if !samples.is_empty() {
            self.handle_audio_samples(&samples).await?;
        }
        Ok(self.take_pending_action())
    }

    pub async fn shutdown(&mut self) {
        self.pseudo.cancel().await;
        self.state = SessionState::Closed;
    }

    /// 受け取った音声をそのまま溜める。区切るのは `finalize` だけ。
    async fn handle_audio_samples(
        &mut self,
        samples: &[f32],
    ) -> std::result::Result<(), SessionError> {
        self.pseudo.append(samples);
        // ASR は一度に 30 秒程度までしか扱えない。クライアントが `finalize` を
        // 送らないまま話し続けた場合の安全弁として、ここでだけ強制的に区切る。
        if self.pseudo.audio_ms() >= self.config.max_utterance_ms as f32 {
            let response = self.finish_utterance(Marker::MaxUtterance).await?;
            self.pending_responses.push(response);
        }
        self.emit_partial().await
    }

    async fn emit_partial(&mut self) -> std::result::Result<(), SessionError> {
        if self.pseudo.has_audio() {
            if let Some(text) = self
                .pseudo
                .advance()
                .await
                .map_err(SessionError::Recognition)?
            {
                let text = self.agreement.observe(&text);
                if !text.is_empty() {
                    // A partial is always the entire current transcription in a
                    // single non-final token; clients replace the prior partial.
                    self.pending_responses.push(AsrResponse {
                        tokens: vec![make_token(&text, false)],
                        final_audio_proc_ms: None,
                        total_audio_proc_ms: None,
                        finished: false,
                        error_code: None,
                        error_type: None,
                        error_message: None,
                        request_id: None,
                        notice_code: None,
                        notice_message: None,
                        backend: None,
                        warmup_after_secs: None,
                    });
                }
            }
        }
        Ok(())
    }

    async fn finish_utterance(
        &mut self,
        marker: Marker,
    ) -> std::result::Result<AsrResponse, SessionError> {
        tracing::info!(reason = marker.reason(), "utterance ended");
        self.dump_utterance();
        let text = self
            .pseudo
            .finalize()
            .await
            .map_err(SessionError::Recognition);
        self.clear_utterance();
        Ok(response_with_marker(marker, &text?))
    }

    /// 認識に渡す直前の音声を書き出す。設定されていなければ何もしない。
    fn dump_utterance(&self) {
        let Some(dir) = self.config.dump_dir.as_deref() else {
            return;
        };
        let samples = self.pseudo.buffer();
        match crate::dump::write_utterance(dir, &samples) {
            Ok(path) => tracing::info!(
                path = %path.display(),
                samples = samples.len(),
                "utterance audio written"
            ),
            Err(error) => tracing::warn!(%error, "failed to write utterance audio"),
        }
    }

    async fn finalize(&mut self) -> std::result::Result<SessionAction, SessionError> {
        if self.pseudo.has_audio() {
            return Ok(SessionAction::response(
                self.finish_utterance(Marker::Fin).await?,
            ));
        }
        Ok(SessionAction::response(response_with_marker(
            Marker::Fin,
            "",
        )))
    }

    async fn finish_connection(&mut self) -> std::result::Result<SessionAction, SessionError> {
        if self.pcm_input.has_pending() {
            return Err(SessionError::protocol(
                400,
                "invalid_audio",
                "PCM フレームが 16 bit 境界で終わっていません",
            ));
        }
        let mut action = SessionAction::default();
        if self.pseudo.has_audio() {
            action
                .responses
                .push(self.finish_utterance(Marker::End).await?);
        }
        action.responses.push(AsrResponse {
            tokens: Vec::new(),
            final_audio_proc_ms: None,
            total_audio_proc_ms: None,
            finished: true,
            error_code: None,
            error_type: None,
            error_message: None,
            request_id: None,
            notice_code: None,
            notice_message: None,
            backend: None,
            warmup_after_secs: None,
        });
        self.state = SessionState::Closing;
        action.close_code = Some(1000);
        Ok(action)
    }

    fn clear_utterance(&mut self) {
        self.pseudo.clear();
        self.agreement.reset();
    }

    fn take_pending_action(&mut self) -> SessionAction {
        SessionAction {
            responses: std::mem::take(&mut self.pending_responses),
            close_code: None,
        }
    }
}

pub fn parse_client_settings(message: &str) -> std::result::Result<AsrConfig, SessionError> {
    let config = serde_json::from_str::<AsrConfig>(message)
        .map_err(|_| SessionError::protocol(400, "invalid_config", "設定 JSON が不正です"))?;
    if config.model.is_empty() {
        return Err(SessionError::protocol(
            400,
            "invalid_config",
            "model は必須です",
        ));
    }
    if config.audio_format != "pcm_s16le" {
        return Err(SessionError::protocol(
            400,
            "invalid_config",
            "audio_format は pcm_s16le のみ対応しています",
        ));
    }
    if config.sample_rate != SAMPLE_RATE {
        return Err(SessionError::protocol(
            400,
            "invalid_config",
            "sample_rate は 16000 のみ対応しています",
        ));
    }
    if config.num_channels != 1 {
        return Err(SessionError::protocol(
            400,
            "invalid_config",
            "num_channels は 1 のみ対応しています",
        ));
    }
    match config.endpoint_mode.as_deref() {
        None | Some("client") => {}
        // このサーバーは終話を判定しない。黙って受理すると、クライアントは
        // 永久に来ない `<end>` を待ち続けることになる。
        Some("server") => {
            return Err(SessionError::protocol(
                400,
                "invalid_config",
                "このサーバーは終話判定を行いません。endpoint_mode=client を指定し、区切りは finalize で送ってください",
            ))
        }
        Some(_) => {
            return Err(SessionError::protocol(
                400,
                "invalid_config",
                "endpoint_mode は client のみ対応しています",
            ))
        }
    }
    if !config.enable_endpoint_detection {
        return Err(SessionError::protocol(
            400,
            "invalid_config",
            "enable_endpoint_detection は true にしてください",
        ));
    }
    Ok(config)
}

fn error_response(code: u32, error_type: &str, message: &str) -> AsrResponse {
    AsrResponse {
        tokens: Vec::new(),
        final_audio_proc_ms: None,
        total_audio_proc_ms: None,
        finished: false,
        error_code: Some(code),
        error_type: Some(error_type.to_string()),
        error_message: Some(message.to_string()),
        request_id: None,
        notice_code: None,
        notice_message: None,
        backend: None,
        warmup_after_secs: None,
    }
}

fn response_with_marker(marker: Marker, text: &str) -> AsrResponse {
    let mut tokens = Vec::new();
    if !text.is_empty() {
        tokens.push(make_token(text, true));
    }
    tokens.push(make_token(marker.token(), true));
    AsrResponse {
        tokens,
        final_audio_proc_ms: None,
        total_audio_proc_ms: None,
        finished: false,
        error_code: None,
        error_type: None,
        error_message: None,
        request_id: None,
        notice_code: None,
        notice_message: None,
        backend: None,
        warmup_after_secs: None,
    }
}

fn make_token(text: &str, is_final: bool) -> AsrToken {
    AsrToken {
        text: text.to_string(),
        start_ms: None,
        end_ms: None,
        confidence: None,
        is_final,
        speaker: None,
        language: None,
        translation_status: None,
        source_language: None,
    }
}

impl Marker {
    fn token(self) -> &'static str {
        match self {
            Self::End => TOKEN_END,
            Self::MaxUtterance => TOKEN_END,
            Self::Fin => TOKEN_FIN,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::End => "endpoint",
            Self::MaxUtterance => "max_utterance",
            Self::Fin => "finalize",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        make_token, parse_client_settings, AsrResponse, Marker, Pcm16Buffer, PseudoStream, Session,
        SessionAction,
    };
    use crate::{asr::Transcriber, audio::SAMPLE_RATE, config::Config};
    use anyhow::Result;
    use otoa_input_protocol::{TOKEN_END, TOKEN_FIN};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };

    struct FakeAsr {
        calls: Arc<AtomicUsize>,
        result: String,
    }

    impl Transcriber for FakeAsr {
        fn transcribe(&self, _samples: &[f32]) -> Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.result.clone())
        }
    }

    fn fake_asr(result: &str) -> (Arc<dyn Transcriber>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let asr = FakeAsr {
            calls: Arc::clone(&calls),
            result: result.to_string(),
        };
        (Arc::new(asr), calls)
    }

    struct RecordingAsr {
        buffers: Arc<Mutex<Vec<Vec<f32>>>>,
    }

    impl Transcriber for RecordingAsr {
        fn transcribe(&self, samples: &[f32]) -> Result<String> {
            self.buffers
                .lock()
                .expect("recording ASR mutex should not be poisoned")
                .push(samples.to_vec());
            Ok("recorded".to_string())
        }
    }

    fn client_config() -> &'static str {
        r#"{"model":"stt-rt-v5","audio_format":"pcm_s16le","sample_rate":16000,"num_channels":1,"enable_endpoint_detection":true}"#
    }

    fn client_endpoint_config() -> &'static str {
        r#"{"model":"stt-rt-v5","audio_format":"pcm_s16le","sample_rate":16000,"num_channels":1,"enable_endpoint_detection":true,"endpoint_mode":"client"}"#
    }

    fn final_text(action: &SessionAction) -> String {
        action
            .responses
            .iter()
            .flat_map(|response| response.tokens.iter())
            .filter(|token| token.is_final && token.text != TOKEN_FIN && token.text != TOKEN_END)
            .map(|token| token.text.as_str())
            .collect()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn partial_is_replaced_not_appended() {
        struct VariableAsr;
        impl Transcriber for VariableAsr {
            fn transcribe(&self, samples: &[f32]) -> Result<String> {
                Ok(if samples.len() < 16_000 {
                    "one".to_string()
                } else {
                    "one two".to_string()
                })
            }
        }
        let mut stream = PseudoStream::new(Arc::new(VariableAsr), 500, true);
        stream.append(&vec![0.0; 8_000]);
        let first = loop {
            if let Some(value) = stream.advance().await.expect("partial should decode") {
                break value;
            }
            tokio::task::yield_now().await;
        };
        stream.append(&vec![0.0; 8_000]);
        let second = loop {
            if let Some(value) = stream.advance().await.expect("partial should decode") {
                break value;
            }
            tokio::task::yield_now().await;
        };
        assert_eq!(first, "one");
        assert_eq!(second, "one two");
        assert_eq!(make_token(&first, false).text, "one");
        assert!(!make_token(&first, false).is_final);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pseudo_stream_off_skips_partials() {
        let (decoder, calls) = fake_asr("final");
        let mut stream = PseudoStream::new(decoder, 500, false);
        stream.append(&vec![0.0; 8_000]);
        assert!(stream
            .advance()
            .await
            .expect("advance should work")
            .is_none());
        assert!(stream
            .advance()
            .await
            .expect("advance should work")
            .is_none());
        assert_eq!(
            stream.finalize().await.expect("final should decode"),
            "final"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn end_token_is_final_and_separate() {
        let response = super::response_with_marker(Marker::End, "こんにちは");
        assert_eq!(response.tokens.len(), 2);
        assert_eq!(response.tokens[0].text, "こんにちは");
        assert!(response.tokens[0].is_final);
        assert_eq!(response.tokens[1].text, TOKEN_END);
        assert!(response.tokens[1].is_final);
        assert!(!response.tokens[0].text.contains(TOKEN_END));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn buffer_cleared_after_end() {
        let (asr, _) = fake_asr("");
        let mut session = Session::new(Config::default(), asr);
        session.pseudo.append(&vec![0.0; 160]);
        let _ = session
            .finish_utterance(Marker::End)
            .await
            .expect("utterance should finish");
        assert!(session.pseudo.buffer().is_empty());
        assert!(!session.pseudo.has_audio());
        assert_eq!(session.state, super::SessionState::Connecting);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn all_received_audio_reaches_the_recognizer_in_order() {
        // サーバーは音声を選り分けない。受け取った順にそのまま ASR へ渡す。
        // 以前はサーバー側の終話判定が「発話中でない」と見なした区間を捨てて
        // おり、2 回目以降の finalize が空になっていた。
        let buffers = Arc::new(Mutex::new(Vec::new()));
        let asr: Arc<dyn Transcriber> = Arc::new(RecordingAsr {
            buffers: Arc::clone(&buffers),
        });
        let mut session = Session::new(Config::default(), asr);
        session
            .handle_text(client_endpoint_config())
            .await
            .expect("configuration should be accepted");

        for value in [1.0_f32, 2.0, 3.0] {
            session
                .handle_audio_samples(&vec![value; 800])
                .await
                .expect("audio should be accepted");
        }
        session.finalize().await.expect("finalize should succeed");

        let buffers = buffers
            .lock()
            .expect("recording ASR mutex should not be poisoned");
        assert_eq!(buffers.len(), 1);
        assert_eq!(buffers[0].len(), 3 * 800);
        assert_eq!(buffers[0][0], 1.0);
        assert_eq!(buffers[0][800], 2.0);
        assert_eq!(buffers[0][1_600], 3.0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn every_finalize_transcribes() {
        // 回帰テスト: 区切りはクライアントの finalize だけが決める。
        // サーバーが自前で発話を終わらせると、そこから音声の蓄積が止まり、
        // 2 回目以降の finalize が空の <fin> だけを返す。そうなると
        // クライアントには貼り付けるテキストが無くなり、実機では
        // 「最初の 1 回しか貼り付かない」という症状になる。
        let (asr, calls) = fake_asr("ここまで");
        let mut session = Session::new(Config::default(), asr);
        session
            .handle_text(client_endpoint_config())
            .await
            .expect("configuration should be accepted");

        for utterance in 0..3 {
            session
                .handle_audio_samples(&vec![0.5; 3_200])
                .await
                .expect("audio should be accepted");
            let action = session.finalize().await.expect("finalize should succeed");
            assert_eq!(
                final_text(&action),
                "ここまで",
                "{utterance} 回目の finalize が空になった"
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn max_utterance_cuts_when_the_client_never_finalizes() {
        // finalize が来ないまま話し続けても、ASR が扱える長さで必ず区切る。
        let (asr, calls) = fake_asr("長い発話");
        let config = Config {
            max_utterance_ms: 1_000,
            ..Config::default()
        };
        let mut session = Session::new(config, asr);
        session
            .handle_text(client_endpoint_config())
            .await
            .expect("configuration should be accepted");

        session
            .handle_audio_samples(&vec![0.5; SAMPLE_RATE as usize])
            .await
            .expect("audio should be accepted");
        let action = session.take_pending_action();
        assert_eq!(final_text(&action), "長い発話");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!session.pseudo.has_audio());
    }

    #[test]
    fn server_side_endpointing_is_rejected_rather_than_silently_ignored() {
        // このサーバーは終話を判定しない。黙って受理すると、クライアントは
        // 永久に来ない <end> を待ち続ける。
        let error = parse_client_settings(
            r#"{"model":"stt-rt-v5","audio_format":"pcm_s16le","sample_rate":16000,"num_channels":1,"enable_endpoint_detection":true,"endpoint_mode":"server"}"#,
        )
        .expect_err("server-side endpointing should be rejected");
        assert!(error.to_string().contains("finalize"));

        let error = parse_client_settings(
            r#"{"model":"stt-rt-v5","audio_format":"pcm_s16le","sample_rate":16000,"num_channels":1,"enable_endpoint_detection":true,"endpoint_mode":"both"}"#,
        )
        .expect_err("unknown endpoint_mode should be rejected");
        assert!(error.to_string().contains("endpoint_mode"));
    }

    #[test]
    fn unknown_config_fields_are_ignored() {
        let settings = parse_client_settings(
            r#"{"model":"stt-rt-v5","audio_format":"pcm_s16le","sample_rate":16000,"num_channels":1,"enable_endpoint_detection":true,"future_field_added_by_client":{"does":"not matter"}}"#,
        )
        .expect("unknown fields should be ignored");
        assert_eq!(settings.model, "stt-rt-v5");
    }

    #[test]
    fn unaligned_chunks_keep_frame_boundary() {
        let source = (0..1_920_i16)
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let mut buffer = Pcm16Buffer::default();
        let mut samples = Vec::new();
        for chunk in [&source[..1], &source[1..18], &source[18..21], &source[21..]] {
            samples.extend(buffer.feed(chunk));
        }
        assert_eq!(samples.len(), 1_920);
        assert_eq!(samples[0], 0.0);
        assert_eq!(samples[1], 1.0 / 32_768.0);
        assert_eq!(samples[1_919], 1_919.0 / 32_768.0);
        assert!(!buffer.has_pending());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_frame_triggers_finished_then_close() {
        let (asr, _) = fake_asr("");
        let mut session = Session::new(Config::default(), asr);
        session
            .handle_text(client_config())
            .await
            .expect("config should be accepted");
        let action = session
            .handle_binary(&[])
            .await
            .expect("empty frame should finish");
        assert_eq!(action.responses.len(), 1);
        assert!(action.responses[0].finished);
        assert_eq!(action.close_code, Some(1000));
        assert_eq!(session.state, super::SessionState::Closing);
        assert_eq!(action.responses[0].tokens.len(), 0);
        assert_eq!(TOKEN_FIN, "<fin>");
        let _: AsrResponse = action.responses[0].clone();
    }
}
