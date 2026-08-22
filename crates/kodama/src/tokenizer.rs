use anyhow::{Context, Result};
use serde_json::Value;
use std::{collections::HashSet, fs, path::Path};

pub struct Tokenizer {
    vocab: Vec<String>,
    special_ids: HashSet<i64>,
}

impl Tokenizer {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read tokenizer {}", path.display()))?;
        let root: Value = serde_json::from_str(&contents)
            .with_context(|| format!("failed to parse tokenizer {}", path.display()))?;
        let vocab_object = root
            .pointer("/model/vocab")
            .and_then(Value::as_object)
            .context("tokenizer model.vocab is not an object")?;
        let max_id = vocab_object
            .values()
            .filter_map(Value::as_u64)
            .max()
            .context("tokenizer model.vocab is empty")? as usize;
        let mut vocab = vec![None; max_id + 1];
        for (token, id) in vocab_object {
            let id = id
                .as_u64()
                .with_context(|| format!("token {token:?} has a non-integer id"))?
                as usize;
            anyhow::ensure!(
                vocab[id].replace(token.clone()).is_none(),
                "duplicate tokenizer vocabulary id {id}"
            );
        }
        let vocab = vocab
            .into_iter()
            .enumerate()
            .map(|(id, token)| token.with_context(|| format!("missing vocabulary id {id}")))
            .collect::<Result<Vec<_>>>()?;
        anyhow::ensure!(
            vocab.len() == 32_000,
            "unexpected tokenizer vocabulary size: {}",
            vocab.len()
        );

        let added_tokens = root
            .get("added_tokens")
            .and_then(Value::as_array)
            .context("tokenizer added_tokens is not an array")?;
        let mut special_ids = HashSet::new();
        for token in added_tokens {
            if token.get("special").and_then(Value::as_bool) == Some(true) {
                let id = token
                    .get("id")
                    .and_then(Value::as_i64)
                    .context("special added token has no integer id")?;
                special_ids.insert(id);
            }
        }
        for id in [0, 1, 2] {
            anyhow::ensure!(special_ids.contains(&id), "special token id {id} is absent");
        }

        Ok(Self { vocab, special_ids })
    }

    pub fn decode(&self, token_ids: &[i64]) -> Result<String> {
        let mut bytes = Vec::new();
        for &id in token_ids {
            if self.special_ids.contains(&id) {
                continue;
            }
            let index = usize::try_from(id).with_context(|| format!("negative token id {id}"))?;
            let token = self
                .vocab
                .get(index)
                .with_context(|| format!("token id {id} is outside the vocabulary"))?;
            if let Some(byte) = byte_fallback(token) {
                bytes.push(byte);
            } else {
                bytes.extend_from_slice(token.replace('\u{2581}', " ").as_bytes());
            }
        }
        if bytes.first() == Some(&b' ') {
            bytes.remove(0);
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

fn byte_fallback(token: &str) -> Option<u8> {
    let bytes = token.as_bytes();
    if bytes.len() != 6 || &bytes[..3] != b"<0x" || bytes[5] != b'>' {
        return None;
    }
    let hex = std::str::from_utf8(&bytes[3..5]).ok()?;
    u8::from_str_radix(hex, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::byte_fallback;

    #[test]
    fn recognizes_only_complete_byte_fallback_tokens() {
        assert_eq!(byte_fallback("<0xE3>"), Some(0xe3));
        assert_eq!(byte_fallback("<0xZZ>"), None);
        assert_eq!(byte_fallback("x<0x41>"), None);
    }
}
