use anyhow::{anyhow, Result};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub const SAMPLE_RATE: u32 = 16_000;
pub const FEATURE_DIM: i32 = 80;
pub const DECODING_METHOD: &str = "greedy_search";
const ENCODER_FILE: &str = "encoder-epoch-99-avg-1.onnx";
const DECODER_FILE: &str = "decoder-epoch-99-avg-1.onnx";
const JOINER_FILE: &str = "joiner-epoch-99-avg-1.onnx";
const TOKENS_FILE: &str = "tokens.txt";

/// A blocking speech-to-text interface used by the session's pseudo stream.
pub trait Transcriber: Send + Sync {
    fn transcribe(&self, samples: &[f32]) -> Result<String>;
}

#[derive(Clone)]
pub struct Asr {
    recognizer: Arc<OfflineRecognizer>,
}

impl Asr {
    pub fn load(model_dir: &Path, threads: usize) -> Result<Self> {
        anyhow::ensure!(
            model_dir.is_dir(),
            "ASR model directory not found: {}",
            model_dir.display()
        );
        anyhow::ensure!(threads > 0, "ASR thread count must be positive");

        let encoder = required_file(model_dir, ENCODER_FILE)?;
        let decoder = required_file(model_dir, DECODER_FILE)?;
        let joiner = required_file(model_dir, JOINER_FILE)?;
        let tokens = required_file(model_dir, TOKENS_FILE)?;

        let mut config = OfflineRecognizerConfig::default();
        config.feat_config.sample_rate = SAMPLE_RATE as i32;
        config.feat_config.feature_dim = FEATURE_DIM;
        config.model_config.transducer = OfflineTransducerModelConfig {
            encoder: Some(encoder.to_string_lossy().into_owned()),
            decoder: Some(decoder.to_string_lossy().into_owned()),
            joiner: Some(joiner.to_string_lossy().into_owned()),
        };
        config.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
        config.model_config.provider = Some("cpu".to_string());
        config.model_config.num_threads = threads as i32;
        config.decoding_method = Some(DECODING_METHOD.to_string());

        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| anyhow!("sherpa-onnx failed to create offline recognizer"))?;
        tracing::info!(
            sample_rate = SAMPLE_RATE,
            feature_dim = FEATURE_DIM,
            "loaded ASR model"
        );
        Ok(Self {
            recognizer: Arc::new(recognizer),
        })
    }

    pub fn transcribe(&self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, samples);
        self.recognizer.decode(&stream);
        let result = stream
            .get_result()
            .ok_or_else(|| anyhow!("sherpa-onnx returned no recognition result"))?;
        Ok(result.text)
    }
}

impl Transcriber for Asr {
    fn transcribe(&self, samples: &[f32]) -> Result<String> {
        Self::transcribe(self, samples)
    }
}

fn required_file(directory: &Path, name: &str) -> Result<PathBuf> {
    let path = directory.join(name);
    anyhow::ensure!(
        path.is_file(),
        "ASR model file not found: {}",
        path.display()
    );
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::{DECODING_METHOD, FEATURE_DIM, SAMPLE_RATE};

    #[test]
    fn recognizer_settings_are_fixed_by_design() {
        assert_eq!(SAMPLE_RATE, 16_000);
        assert_eq!(FEATURE_DIM, 80);
        assert_eq!(DECODING_METHOD, "greedy_search");
    }
}
