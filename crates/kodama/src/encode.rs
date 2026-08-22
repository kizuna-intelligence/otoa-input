use crate::graphs::Graphs;
use anyhow::{Context, Result};
use ort::value::{DynValue, Tensor};

pub fn encode(graphs: &mut Graphs, samples: &[f32]) -> Result<(DynValue, DynValue)> {
    let padded_len = samples.len().div_ceil(80) * 80;
    let mut input_values = vec![0.0_f32; padded_len];
    input_values[..samples.len()].copy_from_slice(samples);
    let mut attention_mask = vec![0_i64; padded_len];
    attention_mask[..samples.len()].fill(1);

    let input_values = Tensor::<f32>::from_array(([1_usize, padded_len], input_values))?;
    let attention_mask = Tensor::<i64>::from_array(([1_usize, padded_len], attention_mask))?;
    let mut outputs = graphs.encoder.run(ort::inputs! {
        "input_values" => input_values,
        "attention_mask" => attention_mask,
    })?;
    let hidden = outputs
        .remove("encoder_hidden_states")
        .context("encoder did not return encoder_hidden_states")?;
    let mask = outputs
        .remove("encoder_attention_mask")
        .context("encoder did not return encoder_attention_mask")?;
    Ok((hidden, mask))
}
