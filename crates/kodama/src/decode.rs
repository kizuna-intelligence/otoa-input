use crate::graphs::Graphs;
use anyhow::{Context, Result};
use ort::{
    memory::Allocator,
    session::SessionInputValue,
    value::{DynValue, Tensor},
};
use std::time::{Duration, Instant};

const DECODER_START_TOKEN_ID: i64 = 1;
const EOS_TOKEN_ID: i64 = 2;
const LAYERS: usize = 10;
const HEADS: usize = 8;
const HEAD_DIM: usize = 64;
const LOGITS_SIZE: usize = 32_768;

pub fn greedy(
    graphs: &mut Graphs,
    hidden: DynValue,
    enc_mask: DynValue,
    max_tokens: usize,
) -> Result<Vec<i64>> {
    Ok(greedy_timed(graphs, hidden, enc_mask, max_tokens)?.token_ids)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct DecodeOutcome {
    pub token_ids: Vec<i64>,
    pub reached_eos: bool,
    pub prefill_elapsed: Duration,
    pub decode_elapsed: Duration,
}

pub(crate) fn greedy_timed(
    graphs: &mut Graphs,
    hidden: DynValue,
    enc_mask: DynValue,
    max_tokens: usize,
) -> Result<DecodeOutcome> {
    let prefill_start = Instant::now();
    let mut prefill_outputs = graphs.prefill.run(ort::inputs! {
        "encoder_hidden_states" => hidden,
    })?;
    let cross = graphs
        .cross_names
        .iter()
        .map(|name| {
            prefill_outputs
                .remove(name)
                .with_context(|| format!("prefill did not return {name}"))
                .map(|value| (name.clone(), value))
        })
        .collect::<Result<Vec<_>>>()?;
    let prefill_elapsed = prefill_start.elapsed();

    let mut past = graphs
        .kv_pairs
        .iter()
        .map(|(input_name, _)| {
            Tensor::<f32>::new(&Allocator::default(), [1_usize, HEADS, 0_usize, HEAD_DIM])
                .map(|value| (input_name.clone(), value.into_dyn()))
        })
        .collect::<ort::Result<Vec<_>>>()?;
    anyhow::ensure!(past.len() == LAYERS * 2, "invalid self-KV cache count");

    let mut current = DECODER_START_TOKEN_ID;
    let mut token_ids = Vec::new();
    let mut reached_eos = false;
    let decode_start = Instant::now();
    for _ in 0..max_tokens {
        let decoder_input = Tensor::<i64>::from_array(([1_usize, 1_usize], vec![current]))?;
        let mut inputs =
            Vec::<(String, SessionInputValue<'_>)>::with_capacity(2 + past.len() + cross.len());
        inputs.push(("decoder_input_ids".to_string(), decoder_input.into()));
        inputs.push(("encoder_attention_mask".to_string(), (&enc_mask).into()));
        inputs.extend(
            past.iter()
                .map(|(name, value)| (name.clone(), value.into())),
        );
        inputs.extend(
            cross
                .iter()
                .map(|(name, value)| (name.clone(), value.into())),
        );

        let mut outputs = graphs.decoder.run(inputs)?;
        let (_, logits) = outputs["logits"].try_extract_tensor::<f32>()?;
        anyhow::ensure!(
            logits.len() == LOGITS_SIZE,
            "unexpected logits length: {}",
            logits.len()
        );
        current = argmax(logits) as i64;

        let next_past = graphs
            .kv_pairs
            .iter()
            .map(|(input_name, output_name)| {
                outputs
                    .remove(output_name)
                    .with_context(|| format!("decoder did not return {output_name}"))
                    .map(|value| (input_name.clone(), value))
            })
            .collect::<Result<Vec<_>>>()?;
        past = next_past;
        if current == EOS_TOKEN_ID {
            reached_eos = true;
            break;
        }
        token_ids.push(current);
    }
    Ok(DecodeOutcome {
        token_ids,
        reached_eos,
        prefill_elapsed,
        decode_elapsed: decode_start.elapsed(),
    })
}

fn argmax(values: &[f32]) -> usize {
    let mut best_index = 0;
    let mut best_value = values[0];
    for (index, &value) in values.iter().enumerate().skip(1) {
        if value > best_value {
            best_index = index;
            best_value = value;
        }
    }
    best_index
}

#[cfg(test)]
mod tests {
    use super::argmax;

    #[test]
    fn argmax_keeps_first_equal_maximum() {
        assert_eq!(argmax(&[1.0, 3.0, 3.0, 2.0]), 1);
    }
}
