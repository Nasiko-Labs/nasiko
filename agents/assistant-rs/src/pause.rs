//! A2A pause events the platform renders as HITL cards.
//!
//! - `TASK_STATE_AUTH_REQUIRED`: `status.message.metadata` carries `provider`, `auth_url`,
//!   `hitl_request_id`, `auth_kind` → Connect button (`pause_question` in `oss/types/src/a2a.rs`).
//! - `TASK_STATE_INPUT_REQUIRED`: metadata may carry `options`/`multi_select`/
//!   `allow_custom_input`/`header` → selectable answers; otherwise a text box.

use std::collections::HashMap;

use a2a::*;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::mcp::{AuthRequired, ToolAsk};

/// One selectable answer.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InputOption {
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// A question for the user.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InputRequest {
    pub question: String,
    #[serde(default)]
    pub options: Vec<InputOption>,
    #[serde(default)]
    pub multi_select: bool,
    /// Defaults to true so the user can always type a free answer.
    #[serde(default = "yes")]
    pub allow_custom_input: bool,
}

fn yes() -> bool {
    true
}

/// Metadata for an auth pause.
pub fn auth_metadata(auth: &AuthRequired) -> HashMap<String, Value> {
    let mut m: HashMap<String, Value> = HashMap::new();
    m.insert("auth_kind".into(), json!("mcp_connector"));
    m.insert("provider".into(), json!(auth.connector));
    if let Some(url) = &auth.auth_url {
        m.insert("auth_url".into(), json!(url));
    }
    if let Some(id) = &auth.hitl_request_id {
        m.insert("hitl_request_id".into(), json!(id));
    }
    if let Some(id) = &auth.connector_id {
        m.insert("connector_id".into(), json!(id));
    }
    m
}

/// Plain-text fallback for clients without a button.
pub fn auth_text(auth: &AuthRequired) -> String {
    match &auth.auth_url {
        Some(url) => format!(
            "Connect {} so I can continue: {url}\nReply when done and I'll pick up where I stopped.",
            auth.connector
        ),
        None => format!(
            "Connect {} in Connectors, then reply and I'll pick up where I stopped.",
            auth.connector
        ),
    }
}

/// Metadata for an input pause; options only when there are any.
pub fn input_metadata(req: &InputRequest) -> HashMap<String, Value> {
    let mut m: HashMap<String, Value> = HashMap::new();
    m.insert("expected_input".into(), json!("text"));
    if !req.options.is_empty() {
        let options: Vec<Value> = req
            .options
            .iter()
            .map(|o| match &o.description {
                Some(d) => json!({"label": o.label, "description": d}),
                None => json!({"label": o.label}),
            })
            .collect();
        m.insert("options".into(), Value::Array(options));
        m.insert("multi_select".into(), json!(req.multi_select));
        m.insert("allow_custom_input".into(), json!(req.allow_custom_input));
    }
    m
}

/// `TASK_STATE_AUTH_REQUIRED` status update.
pub fn auth_required_event(task_id: &str, context_id: &str, auth: &AuthRequired) -> StreamResponse {
    status_event(
        task_id,
        context_id,
        TaskState::AuthRequired,
        &auth_text(auth),
        Some(auth_metadata(auth)),
    )
}

/// `TASK_STATE_INPUT_REQUIRED` status update.
pub fn input_required_event(task_id: &str, context_id: &str, req: &InputRequest) -> StreamResponse {
    status_event(
        task_id,
        context_id,
        TaskState::InputRequired,
        &req.question,
        Some(input_metadata(req)),
    )
}

/// `TASK_STATE_INPUT_REQUIRED` status update for a gateway `TOOL_ASK` HITL pause.
pub fn tool_ask_event(task_id: &str, context_id: &str, ask: &ToolAsk) -> StreamResponse {
    let mut m: HashMap<String, Value> = HashMap::new();
    m.insert("expected_input".into(), json!("text"));
    if let Some(id) = &ask.hitl_request_id {
        m.insert("hitl_request_id".into(), json!(id));
    }
    status_event(
        task_id,
        context_id,
        TaskState::InputRequired,
        &ask.message,
        Some(m),
    )
}

/// Any status update with an agent message. Metadata goes on `status.message`, where the
/// platform reads it first.
pub fn status_event(
    task_id: &str,
    context_id: &str,
    state: TaskState,
    text: &str,
    metadata: Option<HashMap<String, Value>>,
) -> StreamResponse {
    let message: Option<Message> = (!text.is_empty() || metadata.is_some()).then(|| Message {
        message_id: new_message_id(),
        context_id: Some(context_id.into()),
        task_id: Some(task_id.into()),
        role: Role::Agent,
        parts: vec![Part::text(text)],
        metadata,
        extensions: None,
        reference_task_ids: None,
    });
    StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state,
            message,
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(url: Option<&str>) -> AuthRequired {
        AuthRequired {
            connector: "gmail".into(),
            connector_id: None,
            hitl_request_id: Some("h1".into()),
            auth_url: url.map(str::to_owned),
            message: String::new(),
        }
    }

    #[test]
    fn auth_metadata_has_link_provider_and_hitl_id() {
        let m: HashMap<String, Value> = auth_metadata(&auth(Some("https://x")));
        assert_eq!(m["provider"], "gmail");
        assert_eq!(m["auth_url"], "https://x");
        assert_eq!(m["hitl_request_id"], "h1");
    }

    #[test]
    fn auth_metadata_omits_missing_link() {
        assert!(!auth_metadata(&auth(None)).contains_key("auth_url"));
        assert!(auth_text(&auth(None)).contains("Connectors"));
    }

    #[test]
    fn auth_event_serializes_as_auth_required_with_metadata() {
        let ev: StreamResponse = auth_required_event("t", "c", &auth(Some("https://x")));
        let v: Value = serde_json::to_value(&ev).unwrap();
        let s: String = v.to_string();
        assert!(s.contains("TASK_STATE_AUTH_REQUIRED"), "{s}");
        assert!(s.contains("\"auth_url\":\"https://x\""), "{s}");
    }

    #[test]
    fn input_request_defaults_allow_custom_input() {
        let req: InputRequest = serde_json::from_value(json!({
            "question": "Which inbox?", "options": [{"label": "Work"}, {"label": "Personal"}]
        }))
        .unwrap();
        let m: HashMap<String, Value> = input_metadata(&req);
        assert_eq!(m["allow_custom_input"], true);
        assert_eq!(m["options"][1]["label"], "Personal");
    }

    #[test]
    fn input_without_options_is_plain_text() {
        let req: InputRequest = serde_json::from_value(json!({"question": "Name?"})).unwrap();
        assert!(!input_metadata(&req).contains_key("options"));
    }
}
