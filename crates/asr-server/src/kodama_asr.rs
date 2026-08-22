use crate::asr::Transcriber;
use anyhow::Result;
use std::path::Path;

pub struct KodamaAsr(otoa_input_kodama::Kodama);

impl KodamaAsr {
    pub fn load(model_dir: &Path, threads: usize) -> Result<Self> {
        Ok(Self(otoa_input_kodama::Kodama::load(model_dir, threads)?))
    }
}

impl Transcriber for KodamaAsr {
    fn transcribe(&self, samples: &[f32]) -> Result<String> {
        self.0.transcribe(samples)
    }
}
