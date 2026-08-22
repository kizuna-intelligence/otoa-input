//! このワークスペースの ONNX Runtime は sherpa-onnx が静的リンクしている 1 つだけである。
//! ort は自前の実体を持たない (`alternative-backend`) ので、Session を作る前に
//! 必ずここを通して API を登録する。
//!
//! このクレートは `OrtGetApiBase` を定義しない。このクレートを使う最終バイナリは、
//! シンボルを提供する sherpa-onnx にも依存していなければならない。

use anyhow::Result;
use std::sync::OnceLock;

static READY: OnceLock<Result<(), String>> = OnceLock::new();

/// sherpa-onnx が持つ ONNX Runtime API を ort へ冪等に登録する。
pub fn ensure_initialized() -> Result<()> {
    let outcome = READY.get_or_init(|| {
        let api_base = unsafe { ort::sys::OrtGetApiBase() };
        if api_base.is_null() {
            return Err("OrtGetApiBase returned null".into());
        }
        let api = unsafe { ((*api_base).GetApi)(ort::sys::ORT_API_VERSION) };
        if api.is_null() {
            return Err(format!(
                "the bundled ONNX Runtime does not provide ort API version {}",
                ort::sys::ORT_API_VERSION
            ));
        }
        ort::set_api(unsafe { (*api).clone() });
        Ok(())
    });
    outcome.clone().map_err(|message| anyhow::anyhow!(message))
}

#[cfg(test)]
mod tests {
    #[test]
    fn registers_sherpa_onnx_runtime_api() {
        let _link_sherpa = std::mem::size_of::<sherpa_onnx::OfflineRecognizerConfig>();
        super::ensure_initialized().expect("sherpa-onnx should provide a compatible ORT API");
    }
}
