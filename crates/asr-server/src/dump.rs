//! 認識に渡した音声をそのまま WAV で書き出す。
//!
//! 「先頭が欠ける」ような不具合は、欠けているのがクライアントから届いた
//! 音声なのか、認識結果だけなのかを分けないと切り分けられない。ここで
//! 書き出すのは ASR へ渡した配列そのものである。

use crate::audio::SAMPLE_RATE;
use anyhow::{Context, Result};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// `samples` を 16 bit PCM の WAV として `dir` に書き、書いた先を返す。
pub fn write_utterance(dir: &Path, samples: &[f32]) -> Result<PathBuf> {
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create dump directory {}", dir.display()))?;
    let index = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("utterance-{index:05}.wav"));
    let mut file =
        fs::File::create(&path).with_context(|| format!("failed to create {}", path.display()))?;
    file.write_all(&wav_bytes(samples))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes()); // fmt チャンクの長さ
    bytes.extend_from_slice(&1_u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1_u16.to_le_bytes()); // モノラル
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // バイト毎秒
    bytes.extend_from_slice(&2_u16.to_le_bytes()); // ブロック境界
    bytes.extend_from_slice(&16_u16.to_le_bytes()); // ビット深度
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        let clamped = (sample * 32_767.0).clamp(-32_768.0, 32_767.0) as i16;
        bytes.extend_from_slice(&clamped.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::wav_bytes;
    use crate::audio::SAMPLE_RATE;

    #[test]
    fn header_describes_16bit_mono_at_the_session_sample_rate() {
        let bytes = wav_bytes(&[0.0, 1.0, -1.0]);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 1);
        assert_eq!(
            u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
            SAMPLE_RATE
        );
        assert_eq!(u16::from_le_bytes([bytes[34], bytes[35]]), 16);
        assert_eq!(bytes.len(), 44 + 3 * 2);
    }

    #[test]
    fn samples_are_scaled_and_clamped() {
        let bytes = wav_bytes(&[0.0, 1.0, -2.0]);
        assert_eq!(i16::from_le_bytes([bytes[44], bytes[45]]), 0);
        assert_eq!(i16::from_le_bytes([bytes[46], bytes[47]]), 32_767);
        assert_eq!(i16::from_le_bytes([bytes[48], bytes[49]]), -32_768);
    }
}
