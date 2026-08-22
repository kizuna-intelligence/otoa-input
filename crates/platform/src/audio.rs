use crate::Resampler;
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use crossbeam_channel::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// マイク入力デバイス。
#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub card_name: Option<String>,
}

/// 16 kHz / mono / i16 に正規化済みの音声フレーム。
#[derive(Debug)]
pub struct AudioFrame(pub Vec<i16>);

pub struct AudioCapture {
    _stream: cpal::Stream,
}

/// 1 フレームのサンプル数。16 kHz で 120 ms。
pub const FRAME_SAMPLES: usize = 1920;

impl AudioCapture {
    /// 指定デバイス（`None` で既定デバイス）から録音を開始する。
    pub fn start(device_id: Option<&str>, sink: Sender<AudioFrame>) -> Result<Self> {
        let host = cpal::default_host();
        let device = select_device(&host, device_id)?;
        let config = device
            .default_input_config()
            .context("failed to get default microphone configuration")?;
        let actual_device_name = device
            .name()
            .context("failed to get microphone device name")?;
        let requested_device = device_id
            .filter(|value| !value.is_empty())
            .unwrap_or("default");
        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        anyhow::ensure!(channels > 0, "microphone has no channels");

        tracing::info!(
            device = %actual_device_name,
            requested = %requested_device,
            sample_rate,
            channels,
            sample_format = ?config.sample_format(),
            "starting audio capture"
        );

        let state = Arc::new(Mutex::new(CaptureState {
            resampler: Resampler::new(sample_rate),
            frame_buffer: Vec::with_capacity(FRAME_SAMPLES * 2),
            dropped_frames: 0,
            capture_sum_squares: 0,
            capture_peak: 0,
            capture_samples: 0,
            last_capture_log_at: Instant::now(),
        }));
        let stream_config: StreamConfig = config.clone().into();
        let stream = match config.sample_format() {
            SampleFormat::F32 => build_input_stream::<f32>(
                &device,
                &stream_config,
                sample_rate,
                channels,
                state,
                sink,
            )?,
            SampleFormat::I16 => build_input_stream::<i16>(
                &device,
                &stream_config,
                sample_rate,
                channels,
                state,
                sink,
            )?,
            SampleFormat::U16 => build_input_stream::<u16>(
                &device,
                &stream_config,
                sample_rate,
                channels,
                state,
                sink,
            )?,
            sample_format => {
                anyhow::bail!("unsupported microphone sample format: {sample_format:?}")
            }
        };

        stream.play().context("failed to start microphone stream")?;
        Ok(Self { _stream: stream })
    }

    pub fn list_devices() -> Result<Vec<AudioDevice>> {
        let host = cpal::default_host();
        let default_name = host
            .default_input_device()
            .and_then(|device| device.name().ok());
        let card_names = alsa_card_names();
        let mut devices = Vec::new();
        for device in host
            .input_devices()
            .context("failed to enumerate microphone devices")?
        {
            let name = device
                .name()
                .unwrap_or_else(|_| "Unknown microphone".to_string());
            devices.push(AudioDevice {
                id: name.clone(),
                is_default: default_name.as_deref() == Some(name.as_str()),
                card_name: card_name_for_device(&name, &card_names),
                name,
            });
        }
        Ok(devices)
    }
}

fn card_name_for_device(
    name: &str,
    card_names: &std::collections::HashMap<String, String>,
) -> Option<String> {
    let card = name
        .split_once("CARD=")?
        .1
        .split(',')
        .next()
        .map(str::trim)
        .filter(|card| !card.is_empty())?;
    card_names.get(card).cloned()
}

#[cfg(target_os = "linux")]
fn alsa_card_names() -> std::collections::HashMap<String, String> {
    let Ok(cards) = std::fs::read_to_string("/proc/asound/cards") else {
        return std::collections::HashMap::new();
    };

    cards
        .lines()
        .filter_map(|line| {
            let open = line.find('[')?;
            let close = line[open + 1..].find(']')? + open + 1;
            let identifier = line[open + 1..close].trim();
            let name = line[close + 1..].split_once(" - ")?.1.trim();
            (!identifier.is_empty() && !name.is_empty())
                .then(|| (identifier.to_string(), name.to_string()))
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn alsa_card_names() -> std::collections::HashMap<String, String> {
    std::collections::HashMap::new()
}

struct CaptureState {
    resampler: Resampler,
    frame_buffer: Vec<i16>,
    dropped_frames: u64,
    capture_sum_squares: u64,
    capture_peak: i16,
    capture_samples: u64,
    last_capture_log_at: Instant,
}

fn select_device(host: &cpal::Host, device_id: Option<&str>) -> Result<cpal::Device> {
    if let Some(device_id) = device_id.filter(|value| !value.is_empty()) {
        for device in host
            .input_devices()
            .context("failed to enumerate microphone devices")?
        {
            if device.name().map(|name| name == device_id).unwrap_or(false) {
                return Ok(device);
            }
        }
        anyhow::bail!("microphone device not found: {device_id}");
    }

    host.default_input_device()
        .context("no default microphone device found")
}

fn build_input_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    sample_rate: u32,
    channels: usize,
    state: Arc<Mutex<CaptureState>>,
    sink: Sender<AudioFrame>,
) -> Result<cpal::Stream>
where
    T: Sample + SizedSample,
    f32: FromSample<T>,
{
    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let mono = downmix(data, channels);
                let Ok(mut state) = state.lock() else {
                    tracing::warn!("audio state lock was poisoned; dropping input callback");
                    return;
                };

                let mut resampled = Vec::new();
                for &sample in &mono {
                    let sample = (sample * i16::MAX as f32)
                        .round()
                        .clamp(i16::MIN as f32, i16::MAX as f32)
                        as i16;
                    state.capture_sum_squares += u64::from(i32::from(sample).unsigned_abs().pow(2));
                    state.capture_peak = state.capture_peak.max(sample_level(sample));
                }
                state.capture_samples += mono.len() as u64;
                let now = Instant::now();
                if now.duration_since(state.last_capture_log_at)
                    >= std::time::Duration::from_secs(1)
                {
                    state.last_capture_log_at = now;
                    let rms = if state.capture_samples == 0 {
                        0
                    } else {
                        ((state.capture_sum_squares as f64 / state.capture_samples as f64)
                            .sqrt()
                            .round() as i64)
                            .min(i16::MAX as i64) as i16
                    };
                    tracing::debug!(
                        rms,
                        peak = state.capture_peak,
                        sample_rate,
                        channels,
                        stage = "pre-resample",
                        "capture"
                    );
                    state.capture_sum_squares = 0;
                    state.capture_peak = 0;
                    state.capture_samples = 0;
                }
                state.resampler.push(&mono, &mut resampled);
                state.frame_buffer.extend(resampled);
                while state.frame_buffer.len() >= FRAME_SAMPLES {
                    let frame = state.frame_buffer.drain(..FRAME_SAMPLES).collect();
                    if sink.try_send(AudioFrame(frame)).is_err() {
                        state.dropped_frames += 1;
                        tracing::warn!(
                            dropped_frames = state.dropped_frames,
                            "audio frame dropped because controller channel is full or closed"
                        );
                    }
                }
            },
            |error| tracing::error!("audio capture error: {error}"),
            None,
        )
        .context("failed to build microphone input stream")?;
    Ok(stream)
}

fn downmix<T>(data: &[T], channels: usize) -> Vec<f32>
where
    T: Sample,
    f32: FromSample<T>,
{
    if channels <= 1 {
        return data.iter().copied().map(f32::from_sample).collect();
    }

    data.chunks(channels)
        .map(|frame| frame.iter().copied().map(f32::from_sample).sum::<f32>() / channels as f32)
        .collect()
}

fn sample_level(sample: i16) -> i16 {
    sample.unsigned_abs().min(i16::MAX as u16) as i16
}
