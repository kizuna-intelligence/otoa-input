mod decode;
mod encode;
mod graphs;
mod tokenizer;

use anyhow::Result;
use graphs::Graphs;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokenizer::Tokenizer;

const SAMPLE_RATE: usize = 16_000;
const DEFAULT_MAX_TOKENS_PER_SECOND: usize = 20;
const TOKEN_HEADROOM: usize = 32;
const REQUIRED_FILES: &[&str] = &[
    "encoder.onnx",
    "encoder.onnx.data",
    "cross_kv_prefill.onnx",
    "cross_kv_prefill.onnx.data",
    "decoder_step_crosskv.int8a.onnx",
    "tokenizer.json",
];

pub struct Kodama {
    graphs: Arc<Mutex<Graphs>>,
    tokenizer: Arc<Tokenizer>,
    max_tokens_per_second: usize,
}

impl Kodama {
    pub fn load(model_dir: &Path, threads: usize) -> Result<Self> {
        otoa_input_onnx::ensure_initialized()?;
        anyhow::ensure!(
            model_dir.is_dir(),
            "ASR model directory not found: {}",
            model_dir.display()
        );
        anyhow::ensure!(threads > 0, "ASR thread count must be positive");
        for name in REQUIRED_FILES {
            required_file(model_dir, name)?;
        }

        let graphs = Graphs::load(model_dir, threads)?;
        let tokenizer = Tokenizer::load(&model_dir.join("tokenizer.json"))?;
        Ok(Self {
            graphs: Arc::new(Mutex::new(graphs)),
            tokenizer: Arc::new(tokenizer),
            max_tokens_per_second: DEFAULT_MAX_TOKENS_PER_SECOND,
        })
    }

    pub fn transcribe(&self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let max_tokens = samples
            .len()
            .saturating_mul(self.max_tokens_per_second)
            .div_ceil(SAMPLE_RATE)
            .saturating_add(TOKEN_HEADROOM);
        let mut graphs = self
            .graphs
            .lock()
            .map_err(|_| anyhow::anyhow!("kodama graph mutex is poisoned"))?;
        let (hidden, enc_mask) = encode::encode(&mut graphs, samples)?;
        let token_ids = decode::greedy(&mut graphs, hidden, enc_mask, max_tokens)?;
        drop(graphs);
        self.tokenizer.decode(&token_ids)
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
    use super::{decode, encode, Kodama, SAMPLE_RATE, TOKEN_HEADROOM};
    use anyhow::{Context, Result};
    use serde_json::Value;
    use std::{env, fs, path::Path, time::Instant};

    #[test]
    fn missing_model_file_names_the_file() {
        let missing = Path::new("/definitely/not/a/kodama/model");
        let error = match Kodama::load(missing, 2) {
            Ok(_) => panic!("missing model should fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("ASR model directory not found"));
    }

    #[test]
    #[ignore = "requires the 311 MB kodama model and external groundtruth WAV fixtures"]
    fn groundtruth_token_ids_and_timings() -> Result<()> {
        // Keep sherpa-onnx linked into this test binary: otoa-input-onnx resolves
        // OrtGetApiBase from sherpa's statically linked ONNX Runtime.
        let _sherpa_link_anchor = std::mem::size_of::<sherpa_onnx::OfflineRecognizerConfig>();
        let model_dir = env::var("KODAMA_MODEL_DIR").context("KODAMA_MODEL_DIR is required")?;
        let fixture_root =
            env::var("KODAMA_FIXTURE_ROOT").context("KODAMA_FIXTURE_ROOT is required")?;
        let groundtruth_path =
            env::var("KODAMA_GROUNDTRUTH").context("KODAMA_GROUNDTRUTH is required")?;
        let expected: Value = serde_json::from_str(&fs::read_to_string(groundtruth_path)?)?;
        let expected = expected
            .as_object()
            .context("groundtruth root must be an object")?;
        let kodama = Kodama::load(Path::new(&model_dir), 2)?;

        eprintln!("file\tduration_s\tencoder_s\tprefill_s\tdecode_s\ttotal_s\trtf\tpeak_rss_mb");
        for (basename, expected_row) in expected {
            let wav_path = fixture_path(Path::new(&fixture_root), basename);
            let samples = read_pcm16_wav(&wav_path)?;
            let max_tokens = samples
                .len()
                .saturating_mul(kodama.max_tokens_per_second)
                .div_ceil(SAMPLE_RATE)
                .saturating_add(TOKEN_HEADROOM);
            let total_start = Instant::now();
            let mut graphs = kodama
                .graphs
                .lock()
                .map_err(|_| anyhow::anyhow!("kodama graph mutex is poisoned"))?;
            let encoder_start = Instant::now();
            let (hidden, mask) = encode::encode(&mut graphs, &samples)?;
            let encoder_elapsed = encoder_start.elapsed();
            let outcome = decode::greedy_timed(&mut graphs, hidden, mask, max_tokens)?;
            drop(graphs);
            let total_elapsed = total_start.elapsed();

            let mut actual_ids = outcome.token_ids.clone();
            if outcome.reached_eos {
                actual_ids.push(2);
            }
            let expected_ids = expected_row["token_ids"]
                .as_array()
                .context("token_ids must be an array")?
                .iter()
                .map(|id| id.as_i64().context("token id must be an integer"))
                .collect::<Result<Vec<_>>>()?;
            if actual_ids != expected_ids {
                let divergence = actual_ids
                    .iter()
                    .zip(&expected_ids)
                    .position(|(actual, expected)| actual != expected)
                    .unwrap_or(actual_ids.len().min(expected_ids.len()));
                anyhow::bail!(
                    "{basename} diverged at token {divergence}: actual={actual_ids:?}, expected={expected_ids:?}"
                );
            }

            let duration = samples.len() as f64 / SAMPLE_RATE as f64;
            eprintln!(
                "{}\t{:.3}\t{:.6}\t{:.6}\t{:.6}\t{:.6}\t{:.6}\t{:.1}",
                basename,
                duration,
                encoder_elapsed.as_secs_f64(),
                outcome.prefill_elapsed.as_secs_f64(),
                outcome.decode_elapsed.as_secs_f64(),
                total_elapsed.as_secs_f64(),
                total_elapsed.as_secs_f64() / duration,
                peak_rss_mb().unwrap_or_default()
            );
        }
        Ok(())
    }

    fn fixture_path(root: &Path, basename: &str) -> std::path::PathBuf {
        if basename == "demo_speech.wav" {
            root.join("demo").join(basename)
        } else {
            root.join("utterances").join(basename)
        }
    }

    fn read_pcm16_wav(path: &Path) -> Result<Vec<f32>> {
        let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
        anyhow::ensure!(
            bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
            "not a RIFF/WAVE file: {}",
            path.display()
        );
        let mut offset = 12;
        let mut format = None;
        let mut data = None;
        while offset + 8 <= bytes.len() {
            let chunk_id = &bytes[offset..offset + 4];
            let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
            let start = offset + 8;
            let end = start.checked_add(size).context("WAV chunk size overflow")?;
            anyhow::ensure!(end <= bytes.len(), "truncated WAV chunk");
            match chunk_id {
                b"fmt " => format = Some(&bytes[start..end]),
                b"data" => data = Some(&bytes[start..end]),
                _ => {}
            }
            offset = end + (size & 1);
        }
        let format = format.context("WAV has no fmt chunk")?;
        anyhow::ensure!(format.len() >= 16, "short WAV fmt chunk");
        let encoding = u16::from_le_bytes(format[0..2].try_into()?);
        let channels = u16::from_le_bytes(format[2..4].try_into()?);
        let sample_rate = u32::from_le_bytes(format[4..8].try_into()?);
        let bits = u16::from_le_bytes(format[14..16].try_into()?);
        anyhow::ensure!(
            encoding == 1 && channels == 1 && sample_rate == 16_000 && bits == 16,
            "expected mono 16 kHz PCM16 WAV, got format={encoding} channels={channels} rate={sample_rate} bits={bits}"
        );
        let data = data.context("WAV has no data chunk")?;
        anyhow::ensure!(data.len() % 2 == 0, "odd PCM16 data length");
        Ok(data
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_768.0)
            .collect())
    }

    fn peak_rss_mb() -> Option<f64> {
        let status = fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
        let kib = line.split_whitespace().nth(1)?.parse::<f64>().ok()?;
        Some(kib / 1024.0)
    }
}
