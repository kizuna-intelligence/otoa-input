use anyhow::{bail, Context, Result};
use ort::session::Session;
use std::{collections::HashMap, path::Path};

pub struct Graphs {
    pub encoder: Session,
    pub prefill: Session,
    pub decoder: Session,
    /// prefill の出力名。decoder の同名入力と load 時に照合済み。
    pub cross_names: Vec<String>,
    /// decoder の past 入力名と、それに対応する new 出力名。
    pub kv_pairs: Vec<(String, String)>,
}

impl Graphs {
    pub fn load(model_dir: &Path, threads: usize) -> Result<Self> {
        let encoder = load_session(&model_dir.join("encoder.onnx"), threads)?;
        let prefill = load_session(&model_dir.join("cross_kv_prefill.onnx"), threads)?;
        let decoder = load_session(&model_dir.join("decoder_step_crosskv.int8a.onnx"), threads)?;

        let encoder_inputs = input_names(&encoder);
        let encoder_outputs = output_names(&encoder);
        require_exact_names(
            "encoder inputs",
            &encoder_inputs,
            &["input_values", "attention_mask"],
        )?;
        require_exact_names(
            "encoder outputs",
            &encoder_outputs,
            &["encoder_hidden_states", "encoder_attention_mask"],
        )?;
        require_exact_names(
            "prefill inputs",
            &input_names(&prefill),
            &["encoder_hidden_states"],
        )?;

        let prefill_outputs = output_names(&prefill);
        let cross_names = prefill_outputs
            .iter()
            .filter(|name| name.starts_with("cross_k_") || name.starts_with("cross_v_"))
            .cloned()
            .collect::<Vec<_>>();
        anyhow::ensure!(
            cross_names.len() == 20 && cross_names.len() == prefill_outputs.len(),
            "unexpected prefill outputs: expected 20 cross K/V values, got {:?}",
            prefill_outputs
        );

        let decoder_inputs = input_names(&decoder);
        let decoder_outputs = output_names(&decoder);
        for name in &cross_names {
            anyhow::ensure!(
                decoder_inputs.contains(name),
                "prefill output {name:?} has no matching decoder input"
            );
        }

        let past_by_suffix = named_by_suffix(&decoder_inputs, "past_self_")?;
        let new_by_suffix = named_by_suffix(&decoder_outputs, "new_self_")?;
        anyhow::ensure!(
            past_by_suffix.len() == 20 && new_by_suffix.len() == 20,
            "unexpected decoder self-KV interface: {} past inputs, {} new outputs",
            past_by_suffix.len(),
            new_by_suffix.len()
        );
        anyhow::ensure!(
            past_by_suffix.len() == new_by_suffix.len(),
            "decoder past/new self-KV counts differ"
        );

        let mut kv_pairs = Vec::with_capacity(past_by_suffix.len());
        for input_name in decoder_inputs
            .iter()
            .filter(|name| name.starts_with("past_self_"))
        {
            let suffix = input_name
                .strip_prefix("past_self_")
                .expect("filtered by prefix");
            let output_name = new_by_suffix.get(suffix).with_context(|| {
                format!("decoder past input {input_name:?} has no matching new output")
            })?;
            kv_pairs.push((input_name.clone(), output_name.clone()));
        }
        for suffix in new_by_suffix.keys() {
            anyhow::ensure!(
                past_by_suffix.contains_key(suffix),
                "decoder new output {:?} has no matching past input",
                new_by_suffix[suffix]
            );
        }

        let expected_decoder_inputs = 2 + cross_names.len() + kv_pairs.len();
        anyhow::ensure!(
            decoder_inputs.len() == expected_decoder_inputs
                && decoder_inputs.contains(&"decoder_input_ids".to_string())
                && decoder_inputs.contains(&"encoder_attention_mask".to_string()),
            "unexpected decoder inputs: {:?}",
            decoder_inputs
        );
        anyhow::ensure!(
            decoder_outputs.len() == 1 + kv_pairs.len()
                && decoder_outputs.contains(&"logits".to_string()),
            "unexpected decoder outputs: {:?}",
            decoder_outputs
        );

        Ok(Self {
            encoder,
            prefill,
            decoder,
            cross_names,
            kv_pairs,
        })
    }
}

fn load_session(path: &Path, threads: usize) -> Result<Session> {
    Session::builder()
        .context("failed to create ONNX Runtime session builder")?
        .with_intra_threads(threads)
        .map_err(|error| {
            anyhow::anyhow!("failed to configure ONNX Runtime intra-op threads: {error}")
        })?
        .commit_from_file(path)
        .with_context(|| format!("failed to load ONNX graph {}", path.display()))
}

fn input_names(session: &Session) -> Vec<String> {
    session
        .inputs()
        .iter()
        .map(|input| input.name().to_string())
        .collect()
}

fn output_names(session: &Session) -> Vec<String> {
    session
        .outputs()
        .iter()
        .map(|output| output.name().to_string())
        .collect()
}

fn require_exact_names(label: &str, actual: &[String], expected: &[&str]) -> Result<()> {
    if actual.len() != expected.len()
        || expected
            .iter()
            .any(|name| !actual.iter().any(|actual_name| actual_name == name))
    {
        bail!("unexpected {label}: {actual:?}");
    }
    Ok(())
}

fn named_by_suffix(names: &[String], prefix: &str) -> Result<HashMap<String, String>> {
    let mut result = HashMap::new();
    for name in names {
        let Some(suffix) = name.strip_prefix(prefix) else {
            continue;
        };
        if result.insert(suffix.to_string(), name.clone()).is_some() {
            bail!("duplicate {prefix} suffix {suffix:?}");
        }
    }
    Ok(result)
}
