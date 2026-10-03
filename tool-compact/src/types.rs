//! Crate-owned types. The router converts its IR to and from these at the seam, so this crate
//! never depends on `nasiko-llm-router`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool definition: name, optional description, optional JSON Schema for the arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded tool call. `arguments` is the model's JSON object text, passed through verbatim
/// after validation (never re-serialized, so nothing about the call is silently altered).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: String,
}

/// The compact form of a tool list. Holds **only rendered text** — no hidden copy of the
/// original schemas — so [`crate::decode_tools`] genuinely proves the schemas survived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    /// The call-format instructions ([`crate::INSTRUCTIONS`]).
    pub instructions: String,
    /// The definition text (grammar in `README.md`).
    pub definitions: String,
}

impl CompactTools {
    /// The single system-message body: instructions, a newline, then the definitions.
    pub fn system_text(&self) -> String {
        format!("{}\n{}", self.instructions, self.definitions)
    }
}

/// How parameter descriptions are rendered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DescriptionPolicy {
    /// Keep every description (whitespace collapsed). The default.
    #[default]
    Verbatim,
    /// Drop a *parameter* description only when every non-stopword in it already appears in
    /// the parameter or tool name ("Event title" on `title`). Tool descriptions are never
    /// dropped. A measured, opt-in ablation.
    DropRedundant,
}

/// Encoder options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EncodeOptions {
    pub descriptions: DescriptionPolicy,
}

/// Decoder options. The default is the strict brief grammar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecodeOptions {
    /// Also accept `<<NAME {…}>>` (no `call` keyword) when `NAME` is exactly one of the tools.
    /// Live models frequently drop the keyword; the call is otherwise decoded and validated
    /// exactly as `<<call NAME {…}>>`. An unknown `<<name` stays text.
    pub bare_tool_markers: bool,
}

/// One unit of decoder output, in stream order.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Plain text outside any call marker.
    Text(String),
    /// A complete, validated call.
    Call(ToolCall),
}

/// A fully decoded response.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Decoded {
    /// The text outside call markers. Empty when the response was only calls (whitespace and
    /// code-fence lines around calls are not "text").
    pub text: String,
    pub calls: Vec<ToolCall>,
}

impl Decoded {
    pub(crate) fn from_events(events: Vec<Event>) -> Self {
        let mut out = Decoded::default();
        for event in events {
            match event {
                Event::Text(t) => out.text.push_str(&t),
                Event::Call(c) => out.calls.push(c),
            }
        }
        if !out.calls.is_empty() && is_only_scaffolding(&out.text) {
            out.text.clear();
        }
        out
    }
}

/// Whether `text` holds nothing but whitespace and code-fence lines — what a model leaves
/// around calls it wrapped in a fenced block.
pub(crate) fn is_only_scaffolding(text: &str) -> bool {
    text.lines().all(|l| {
        let t = l.trim();
        t.is_empty() || t.starts_with("```")
    })
}
