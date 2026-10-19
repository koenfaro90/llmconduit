//! Small request-list projection of client-supplied generation settings.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct InboundSettings {
    pub reasoning_effort: Option<Value>,
    pub thinking: Option<Value>,
    pub clear_thinking: Option<Value>,
    pub max_tokens: Option<Value>,
}

impl InboundSettings {
    /// Read the stored inbound skeleton, never model/profile defaults or upstream edits.
    pub fn from_event(payload: Option<&str>) -> Self {
        let parsed = payload
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .and_then(|envelope| envelope.get("content")?.as_str().map(str::to_owned))
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        parsed.as_ref().map(Self::from_body).unwrap_or_default()
    }

    pub fn from_body(body: &Value) -> Self {
        let field = |paths: &[&str]| paths.iter().find_map(|path| body.pointer(path)).cloned();
        Self {
            reasoning_effort: field(&["/reasoning_effort", "/reasoning/effort"]),
            thinking: field(&[
                "/thinking",
                "/chat_template_kwargs/thinking",
                "/chat_template_kwargs/enable_thinking",
            ]),
            clear_thinking: field(&["/clear_thinking", "/chat_template_kwargs/clear_thinking"]),
            max_tokens: field(&[
                "/max_completion_tokens",
                "/max_tokens",
                "/max_output_tokens",
            ]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inbound_settings_round_trip_and_explicit_values_win() {
        let value = InboundSettings::from_body(&json!({
            "reasoning_effort":"high", "reasoning":{"effort":"low"},
            "thinking":false, "clear_thinking":false,
            "chat_template_kwargs":{"enable_thinking":true,"clear_thinking":true},
            "max_completion_tokens":8192,"max_tokens":100
        }));
        assert_eq!(value.reasoning_effort, Some(json!("high")));
        assert_eq!(value.thinking, Some(json!(false)));
        assert_eq!(value.clear_thinking, Some(json!(false)));
        assert_eq!(value.max_tokens, Some(json!(8192)));
        assert_eq!(
            serde_json::from_str::<InboundSettings>(&serde_json::to_string(&value).unwrap())
                .unwrap(),
            value
        );
    }

    #[test]
    fn nested_protocol_settings_and_absence_are_distinct() {
        let body = json!({"reasoning":{"effort":"medium"},"chat_template_kwargs":{"enable_thinking":false,"clear_thinking":true},"max_output_tokens":2048});
        let envelope = json!({"content":body.to_string()}).to_string();
        let settings = InboundSettings::from_event(Some(&envelope));
        assert_eq!(settings.reasoning_effort, Some(json!("medium")));
        assert_eq!(settings.thinking, Some(json!(false)));
        assert_eq!(settings.clear_thinking, Some(json!(true)));
        assert_eq!(settings.max_tokens, Some(json!(2048)));
        assert_eq!(
            InboundSettings::from_event(Some("invalid")),
            InboundSettings::default()
        );
        assert_eq!(
            InboundSettings::from_body(&json!({})),
            InboundSettings::default()
        );
    }
}
