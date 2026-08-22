use crate::{AsrError, AsrEvent};

/// server から届く 1 レスポンス。
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct AsrResponse {
    #[serde(default)]
    pub tokens: Vec<AsrToken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_audio_proc_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_audio_proc_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub finished: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct AsrToken {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub is_final: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_language: Option<String>,
}

pub const TOKEN_END: &str = "<end>";
pub const TOKEN_FIN: &str = "<fin>";

impl AsrToken {
    pub fn is_endpoint(&self) -> bool {
        self.text == TOKEN_END
    }

    pub fn is_finalize_marker(&self) -> bool {
        self.text == TOKEN_FIN
    }

    /// 本文に連結してよい通常トークンか。
    pub fn is_text(&self) -> bool {
        !self.text.is_empty() && !self.is_endpoint() && !self.is_finalize_marker()
    }
}

pub(crate) fn parse_response(text: &str) -> Result<Vec<AsrEvent>, AsrError> {
    let response: AsrResponse =
        serde_json::from_str(text).map_err(|error| AsrError::Decode(error.to_string()))?;

    if let Some(error_type) = response.error_type {
        return Ok(vec![AsrEvent::Failed(AsrError::Server {
            code: response.error_code.unwrap_or_default(),
            error_type,
            message: response.error_message.unwrap_or_default(),
            request_id: response.request_id,
        })]);
    }

    let mut final_tokens = Vec::new();
    let mut partial_tokens = Vec::new();
    let mut endpoint = false;
    let mut finalize_done = false;

    for token in response.tokens {
        if token.is_endpoint() {
            endpoint = true;
        } else if token.is_finalize_marker() {
            finalize_done = true;
        } else if token.is_text() {
            if token.is_final {
                final_tokens.push(token);
            } else {
                partial_tokens.push(token);
            }
        }
    }

    let mut events = Vec::new();
    if !final_tokens.is_empty() {
        events.push(AsrEvent::FinalText(final_tokens));
    }
    events.push(AsrEvent::PartialText(partial_tokens));
    if finalize_done {
        events.push(AsrEvent::FinalizeDone);
    }
    if endpoint {
        events.push(AsrEvent::Endpoint);
    }
    if response.finished {
        events.push(AsrEvent::Finished);
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::parse_response;
    use crate::{AsrError, AsrEvent};

    fn token_texts(event: &AsrEvent) -> Vec<&str> {
        match event {
            AsrEvent::FinalText(tokens) | AsrEvent::PartialText(tokens) => {
                tokens.iter().map(|token| token.text.as_str()).collect()
            }
            _ => Vec::new(),
        }
    }

    #[test]
    fn final_and_partial_are_split() {
        let events = parse_response(
            r#"{"tokens":[{"text":"確定","is_final":true},{"text":"未確定","is_final":false}]}"#,
        )
        .expect("response should parse");
        assert_eq!(events.len(), 2);
        assert_eq!(token_texts(&events[0]), vec!["確定"]);
        assert_eq!(token_texts(&events[1]), vec!["未確定"]);
    }

    #[test]
    fn end_token_is_not_text() {
        let events = parse_response(
            r#"{"tokens":[{"text":"本文","is_final":true},{"text":"<end>","is_final":true}]}"#,
        )
        .expect("response should parse");
        assert_eq!(token_texts(&events[0]), vec!["本文"]);
        assert!(matches!(events.last(), Some(AsrEvent::Endpoint)));
    }

    #[test]
    fn fin_token_is_not_text() {
        let events = parse_response(
            r#"{"tokens":[{"text":"本文","is_final":true},{"text":"<fin>","is_final":true}]}"#,
        )
        .expect("response should parse");
        assert_eq!(token_texts(&events[0]), vec!["本文"]);
        assert!(events
            .iter()
            .any(|event| matches!(event, AsrEvent::FinalizeDone)));
        assert!(!events
            .iter()
            .any(|event| token_texts(event).contains(&"<fin>")));
    }

    #[test]
    fn event_order_final_before_endpoint() {
        let events = parse_response(
            r#"{"tokens":[{"text":"本文","is_final":true},{"text":"<end>","is_final":true}]}"#,
        )
        .expect("response should parse");
        let final_index = events
            .iter()
            .position(|event| matches!(event, AsrEvent::FinalText(_)))
            .expect("final event");
        let endpoint_index = events
            .iter()
            .position(|event| matches!(event, AsrEvent::Endpoint))
            .expect("endpoint event");
        assert!(final_index < endpoint_index);
    }

    #[test]
    fn empty_partial_is_emitted() {
        let events = parse_response(r#"{"tokens":[{"text":"本文","is_final":true}]}"#)
            .expect("response should parse");
        assert!(events
            .iter()
            .any(|event| matches!(event, AsrEvent::PartialText(tokens) if tokens.is_empty())));
    }

    #[test]
    fn error_frame_maps_to_server_error() {
        let events = parse_response(
            r#"{"error_code":401,"error_type":"authentication","error_message":"bad key","request_id":"req-1"}"#,
        )
        .expect("response should parse");
        match events.as_slice() {
            [AsrEvent::Failed(AsrError::Server {
                code,
                error_type,
                message,
                request_id,
            })] => {
                assert_eq!(*code, 401);
                assert_eq!(error_type, "authentication");
                assert_eq!(message, "bad key");
                assert_eq!(request_id.as_deref(), Some("req-1"));
            }
            other => panic!("unexpected events: {other:?}"),
        }
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let events = parse_response(
            r#"{"tokens":[{"text":"ok","is_final":true,"new_field":123}],"future_field":true}"#,
        )
        .expect("unknown fields should be ignored");
        assert_eq!(token_texts(&events[0]), vec!["ok"]);
    }

    #[test]
    fn finished_flag_emits_finished() {
        let events = parse_response(r#"{"finished":true}"#).expect("response should parse");
        assert!(matches!(events.last(), Some(AsrEvent::Finished)));
    }
}
