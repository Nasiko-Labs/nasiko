use super::Case;
use anyhow::Result;
use nasiko_tool_compact::{ToolDef, encode_tools};
use serde_json::{Value, json};

const INSTRUCTIONS: &str = "Today:2026-10-02 Asia/Kolkata. Use listed CALLs as needed, otherwise answer normally. ? optional; ! no extra keys; = enum.";

pub(super) fn build(case: &Case, tools: &[ToolDef]) -> Result<(Value, bool)> {
    let mut native = json!({"messages":case.messages,"tools":tools});
    for key in [
        "tool_choice",
        "max_tokens",
        "top_p",
        "parallel_tool_calls",
        "response_format",
    ] {
        if let Some(value) = case.extra.get(key) {
            native[key] = value.clone();
        }
    }
    let unsafe_choice = case
        .extra
        .get("tool_choice")
        .is_some_and(|choice| choice != "auto");
    let history = case.messages.iter().any(|message| {
        message["role"] == "tool"
            || message
                .get("tool_calls")
                .is_some_and(|calls| !calls.is_null())
    });
    if tools.is_empty() || unsafe_choice || history || case.extra.contains_key("response_format") {
        return Ok((native, false));
    }
    let Ok(compact) = encode_tools(tools) else {
        return Ok((native, false));
    };
    let mut messages = case.messages.clone();
    let position = messages
        .iter()
        .take_while(|message| message["role"] == "system")
        .count();
    messages.insert(
        position,
        json!({"role":"system","content":format!("{INSTRUCTIONS}\n{}",compact.rendered)}),
    );
    let Some(object) = native.as_object_mut() else {
        anyhow::bail!("native request must be object")
    };
    object.remove("tools");
    object.remove("tool_choice");
    object.remove("parallel_tool_calls");
    object.insert("messages".into(), json!(messages));
    Ok((native, true))
}
