use super::Case;
use anyhow::Result;
use nasiko_tool_compact::{SchemaKind, SchemaNode, ToolDef, analyze_tools, encode_tools};
use serde_json::{Value, json};

const INSTRUCTIONS: &str =
    "Today:2026-10-02 Asia/Kolkata. Call <<call TOOL_NAME JSON_OBJECT>> or answer.";

pub(super) fn native(case: &Case, tools: &[ToolDef]) -> Value {
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
    native
}

pub(super) fn build(case: &Case, tools: &[ToolDef]) -> Result<(Value, bool)> {
    let mut native = native(case, tools);
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
    let single_call_only = case.extra.get("parallel_tool_calls") == Some(&Value::Bool(false));
    if tools.is_empty()
        || unsafe_choice
        || history
        || single_call_only
        || case.extra.contains_key("response_format")
    {
        return Ok((native, false));
    }
    let Ok(compact) = encode_tools(tools) else {
        return Ok((native, false));
    };
    let mut notation = Notation::default();
    for tool in analyze_tools(tools)? {
        if let Some(node) = &tool.parameters {
            notation.visit(node);
        }
    }
    let legend = notation.legend();
    let mut messages = case.messages.clone();
    let position = messages
        .iter()
        .take_while(|message| message["role"] == "system")
        .count();
    messages.insert(
        position,
        json!({"role":"system","content":format!("{INSTRUCTIONS}{legend}\n{}",compact.rendered)}),
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

#[derive(Default)]
struct Notation {
    optional: bool,
    closed: bool,
    enums: bool,
    descriptions: bool,
}

impl Notation {
    fn visit(&mut self, node: &SchemaNode) {
        self.enums |= node.enum_values.is_some();
        self.descriptions |= node.description.is_some();
        match &node.kind {
            SchemaKind::Object {
                properties,
                additional_properties,
            } => {
                self.closed |= !additional_properties;
                for property in properties.values() {
                    self.optional |= !property.required;
                    self.visit(&property.schema);
                }
            }
            SchemaKind::Array(items) => self.visit(items),
            _ => {}
        }
    }

    fn legend(&self) -> String {
        let parts: Vec<_> = [
            (self.optional, "? optional"),
            (self.closed, "! no extra keys"),
            (self.enums, "= enum"),
            (self.descriptions, "(text) description"),
        ]
        .into_iter()
        .filter_map(|(used, text)| used.then_some(text))
        .collect();
        if parts.is_empty() {
            String::new()
        } else {
            format!(" {}.", parts.join("; "))
        }
    }
}
