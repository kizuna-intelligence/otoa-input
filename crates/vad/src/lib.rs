use anyhow::{Context, Result};
use ort::{session::Session, value::Tensor};
use std::path::Path;

pub const VAD_SAMPLE_RATE: u32 = 16_000;
pub const VAD_HOP: usize = 512;
pub const VAD_CONTEXT: usize = 64;
/// 1 フレームの長さ（ミリ秒）。512 / 16000。
pub const VAD_FRAME_MS: u32 = 32;

/// Silero VAD の ONNX wrapper semantics を保ったストリーミング推論器。
pub struct SileroVad {
    session: Session,
    input_names: [String; 3],
    output_names: [String; 2],
    state: Vec<f32>,
    context: Vec<f32>,
    carry: Vec<i16>,
}

/// バイナリへ埋め込んだ Silero VAD。
///
/// 外部ファイルにすると、配布物がバイナリ 1 つで完結しなくなる。
/// 2.3 MB なので埋め込む方が扱いやすい。
pub const BUNDLED_MODEL: &[u8] = include_bytes!("../../../resources/models/silero_vad.onnx");

/// `SessionBuilder` の設定は失敗すると builder ごと返す型になっている。
/// anyhow へ寄せるための小さな受け皿。
fn tune(
    result: Result<
        ort::session::builder::SessionBuilder,
        ort::Error<ort::session::builder::SessionBuilder>,
    >,
    what: &str,
) -> Result<ort::session::builder::SessionBuilder> {
    result.map_err(|error| anyhow::anyhow!("failed to set VAD {what}: {error}"))
}

impl SileroVad {
    /// 埋め込んだモデルを読み込む。外部ファイルは要らない。
    pub fn bundled() -> Result<Self> {
        Self::from_model_bytes(BUNDLED_MODEL, "<bundled>")
    }

    /// 外部の onnx を読み込む。差し替えて試すときに使う。
    pub fn from_model_path(path: &Path) -> Result<Self> {
        anyhow::ensure!(path.is_file(), "VAD model not found: {}", path.display());
        let bytes = std::fs::read(path)
            .with_context(|| format!("failed to read VAD model {}", path.display()))?;
        Self::from_model_bytes(&bytes, &path.display().to_string())
    }

    fn from_model_bytes(bytes: &[u8], origin: &str) -> Result<Self> {
        otoa_input_onnx::ensure_initialized()?;
        // **スレッドを 1 本に絞り、スピンを止める。**
        // ONNX Runtime の既定は intra_op スレッド数がコア数で、しかも
        // 待つ間スピンする。VAD は 512 サンプル（32ms）ごとに走る小さなモデルで、
        // 常時鳴らし続けるものである。既定のままだと、待受しているだけで
        // 何コアも焼く（20 コアの機械で 6 コアを占有するのを実測した）。
        // **スレッドを 1 本に絞り、スピンを止める。**
        // ONNX Runtime の既定は intra_op スレッド数がコア数で、しかも
        // 待つ間スピンする。VAD は 512 サンプル（32ms）ごとに走る小さなモデルで、
        // 常時鳴らし続けるものである。既定のままだと、待受しているだけで
        // 何コアも焼く（20 コアの機械で 6 コアを占有するのを実測した）。
        let builder =
            Session::builder().context("failed to create ONNX Runtime session builder")?;
        let builder = tune(builder.with_intra_threads(1), "intra-op threads")?;
        let builder = tune(builder.with_intra_op_spinning(false), "intra-op spinning")?;
        let builder = tune(builder.with_inter_threads(1), "inter-op threads")?;
        let builder = tune(builder.with_inter_op_spinning(false), "inter-op spinning")?;
        let mut builder = builder;
        let session = builder
            .commit_from_memory(bytes)
            .with_context(|| format!("failed to load VAD model {origin}"))?;

        let input_names = session
            .inputs()
            .iter()
            .map(|input| input.name())
            .map(str::to_string)
            .collect::<Vec<_>>();
        let output_names = session
            .outputs()
            .iter()
            .map(|output| output.name())
            .map(str::to_string)
            .collect::<Vec<_>>();
        tracing::debug!(?input_names, ?output_names, "loaded Silero VAD model IO");
        anyhow::ensure!(
            session.inputs().len() == 3 && session.outputs().len() == 2,
            "unexpected Silero VAD model IO: {} inputs, {} outputs",
            session.inputs().len(),
            session.outputs().len()
        );
        let input_names = input_names
            .try_into()
            .map_err(|_| anyhow::anyhow!("failed to retain Silero VAD input names"))?;
        let output_names = output_names
            .try_into()
            .map_err(|_| anyhow::anyhow!("failed to retain Silero VAD output names"))?;

        Ok(Self {
            session,
            input_names,
            output_names,
            state: vec![0.0; 2 * 128],
            context: vec![0.0; VAD_CONTEXT],
            carry: Vec::new(),
        })
    }

    /// 16 kHz mono i16 を渡す。内部で 512 サンプルごとに推論し、
    /// 各フレームの発話確率を `out` へ順に push する。
    /// 512 に満たない端数は内部に持ち越す。
    pub fn push(&mut self, samples: &[i16], out: &mut Vec<f32>) -> Result<()> {
        self.carry.extend_from_slice(samples);
        while self.carry.len() >= VAD_HOP {
            let chunk = self.carry.drain(..VAD_HOP).collect::<Vec<_>>();
            out.push(self.infer_chunk(&chunk)?);
        }
        Ok(())
    }

    /// 再帰状態・コンテキスト・端数をすべて捨てる。
    pub fn reset(&mut self) {
        self.state.fill(0.0);
        self.context.fill(0.0);
        self.carry.clear();
    }

    fn infer_chunk(&mut self, chunk: &[i16]) -> Result<f32> {
        anyhow::ensure!(chunk.len() == VAD_HOP, "invalid VAD chunk length");
        let mut input = Vec::with_capacity(VAD_CONTEXT + VAD_HOP);
        input.extend_from_slice(&self.context);
        input.extend(chunk.iter().map(|sample| *sample as f32 / i16::MAX as f32));
        let next_context = input[input.len() - VAD_CONTEXT..].to_vec();

        let input_tensor = Tensor::<f32>::from_array(([1usize, VAD_CONTEXT + VAD_HOP], input))?;
        let state_tensor = Tensor::<f32>::from_array(([2usize, 1, 128], self.state.clone()))?;
        let sample_rate_tensor = Tensor::<i64>::from_array(((), vec![VAD_SAMPLE_RATE as i64]))?;
        let outputs = self.session.run(ort::inputs! {
            self.input_names[0].clone() => input_tensor,
            self.input_names[1].clone() => state_tensor,
            self.input_names[2].clone() => sample_rate_tensor,
        })?;

        let probability = outputs[self.output_names[0].as_str()]
            .try_extract_tensor::<f32>()?
            .1
            .iter()
            .next()
            .copied()
            .context("Silero VAD returned an empty probability tensor")?;
        let next_state = outputs[self.output_names[1].as_str()]
            .try_extract_tensor::<f32>()?
            .1
            .to_vec();
        anyhow::ensure!(
            next_state.len() == self.state.len(),
            "unexpected Silero VAD state length: {}",
            next_state.len()
        );
        self.state = next_state;
        self.context.copy_from_slice(&next_context);
        Ok(probability)
    }
}

#[cfg(test)]
mod tests {
    use super::{SileroVad, VAD_CONTEXT, VAD_FRAME_MS, VAD_HOP, VAD_SAMPLE_RATE};
    use std::path::PathBuf;

    fn model_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources/models/silero_vad.onnx")
    }

    fn vad_or_skip() -> Option<SileroVad> {
        let path = model_path();
        if !path.exists() {
            return None;
        }
        Some(SileroVad::from_model_path(&path).expect("bundled VAD model should load"))
    }

    #[test]
    fn hop_and_context_constants() {
        let _link_sherpa = std::mem::size_of::<sherpa_onnx::OfflineRecognizerConfig>();
        assert_eq!(VAD_SAMPLE_RATE, 16_000);
        assert_eq!(VAD_HOP, 512);
        assert_eq!(VAD_CONTEXT, 64);
        assert_eq!(VAD_FRAME_MS, 32);
    }

    #[test]
    fn push_buffers_partial_frames() {
        let Some(mut vad) = vad_or_skip() else { return };
        let mut out = Vec::new();
        for _ in 0..6 {
            vad.push(&[0; 100], &mut out)
                .expect("inference should succeed");
        }
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn push_emits_one_prob_per_hop() {
        let Some(mut vad) = vad_or_skip() else { return };
        let mut out = Vec::new();
        vad.push(&[0; VAD_HOP * 4], &mut out)
            .expect("inference should succeed");
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn reset_clears_carry() {
        let Some(mut vad) = vad_or_skip() else { return };
        let mut out = Vec::new();
        vad.push(&[0; 100], &mut out)
            .expect("inference should succeed");
        vad.reset();
        vad.push(&[0; 412], &mut out)
            .expect("inference should succeed");
        assert!(out.is_empty());
    }
    #[test]
    fn dump_file_probability() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../resources/models/silero_vad.onnx");
        if !path.exists() {
            eprintln!("no model");
            return;
        }
        let pcm = match std::fs::read("/tmp/vad_dump.pcm") {
            Ok(v) => v,
            Err(_) => {
                eprintln!("no dump");
                return;
            }
        };
        let samples: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let mut vad = crate::SileroVad::from_model_path(&path).unwrap();
        let mut out = Vec::new();
        vad.push(&samples, &mut out).unwrap();
        let max = out.iter().cloned().fold(0.0f32, f32::max);
        let over = out.iter().filter(|p| **p >= 0.5).count();
        eprintln!(
            "RUST VAD: frames={} max_prob={:.6} over0.5={}",
            out.len(),
            max,
            over
        );
        eprintln!("first 10: {:?}", &out[..out.len().min(10)]);
    }
}
