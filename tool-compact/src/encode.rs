//! Tool list ⇄ compact prompt text, and the bypass decision.

use std::collections::BTreeSet;
use std::fmt;

use serde_json::{Map, Value};

use crate::calls::render_call;
use crate::error::{Error, Result};
use crate::schema::{Node, Obj, Ty, is_name, params_from_schema, params_to_schema};
use crate::signature;
use crate::types::{ToolCall, ToolDef};

/// Appended once, after the tool lines.
pub const CALL_INSTRUCTION: &str = "To call a tool, emit: <<call NAME {json args}>>";

/// Always appended after [`CALL_INSTRUCTION`], on its own line.
pub const NOTES: &str = "? = optional; args are one JSON object";

/// Appended to [`NOTES`] only when some tool has a `datetime` field. The example is deliberately
/// unrelated to any eval answer.
pub const DATETIME_NOTE: &str = "; datetime = RFC 3339 with offset, e.g. 2026-01-31T09:30:00+01:00";

/// [`Instructions::Terse`] replacement for [`CALL_INSTRUCTION`]. Live runs showed two failures
/// it targets: answering in prose instead of calling, and ending the last call with `}` instead
/// of `>>`.
pub const TERSE_CALL_INSTRUCTION: &str =
    "Call tools ONLY as <<call NAME {json args}>>, each ending >>";

/// [`Instructions::Terse`] replacement for [`NOTES`].
/// `?` marks an optional field; the note says both that and "leave it out unless the user gave
/// it" in four words.
pub const TERSE_NOTES: &str = "?=omit unless given";

/// [`Instructions::Terse`] replacement for [`DATETIME_NOTE`], under the same condition.
pub const TERSE_DATETIME_NOTE: &str = "; datetime=RFC3339+offset";

/// [`Instructions::Balanced`]: call format and notes on one line. The placeholder has no braces:
/// `{json args}` was read as a wrapper around the JSON, and three live runs on two models closed
/// a call with an extra `}`. "never ask" targets replies that asked a question instead of
/// calling; "omit unless given" is left out because it seemed to provoke those questions. No
/// characters that need escaping inside a JSON request body.
///
/// "Always call tools directly, never ask" nudges the model toward calling, which changes
/// behaviour beyond the format. It is opt-in: the library default stays [`Instructions::Minimal`].
pub const BALANCED_INSTRUCTION: &str =
    "Always call tools directly, never ask: <<call NAME ARGS>> (ARGS: JSON object). ?=optional";

/// [`Instructions::Balanced`] replacement for [`DATETIME_NOTE`], under the same condition.
pub const BALANCED_DATETIME_NOTE: &str = ". datetime=RFC3339+offset";

/// Prefix of the optional sample-call line ([`Instructions::Example`]).
const EXAMPLE_PREFIX: &str = "Example: ";

/// Separates the tool lines from the instructions.
const TRAILER_SEPARATOR: &str = "\n\n";

/// How much call-format guidance to append after the tool lines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Instructions {
    /// [`CALL_INSTRUCTION`] and the [`NOTES`] line.
    #[default]
    Minimal,
    /// [`Instructions::Minimal`] plus one line with a sample call to the first tool, built from
    /// its required fields. Costs tokens; may help weaker models follow the format.
    Example,
    /// [`CALL_INSTRUCTION`] and the shortest notes line ([`TERSE_NOTES`]), for the fewest
    /// tokens.
    Terse,
    /// One instruction line ([`BALANCED_INSTRUCTION`]) with a brace-free placeholder and
    /// "never ask", paid for by writing parameter descriptions as `(text)` instead of JSON
    /// strings where that reads back exactly.
    Balanced,
}

impl Instructions {
    /// Every variant, for callers (and [`decode_tools`]) that need to try each one.
    pub const ALL: [Self; 4] = [Self::Minimal, Self::Example, Self::Terse, Self::Balanced];

    /// Parse `minimal` / `example` / `terse` / `balanced` (for a caller holding a config string).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "minimal" => Some(Self::Minimal),
            "example" => Some(Self::Example),
            "terse" => Some(Self::Terse),
            "balanced" => Some(Self::Balanced),
            _ => None,
        }
    }
}

/// Options for [`encode_tools_with`].
#[derive(Debug, Clone, Copy, Default)]
pub struct EncodeOptions<'a> {
    /// The request's `tool_choice`; see [`tool_choice_allows_compact`].
    pub tool_choice: Option<&'a Value>,
    /// Call-format guidance.
    pub instructions: Instructions,
}

/// The result of [`encode_tools`].
#[derive(Debug, Clone, PartialEq)]
pub enum CompactTools {
    /// The tool list fits the grammar. Send `prompt` (e.g. as a system message) *instead of*
    /// native `tools`, and decode the reply with [`crate::decode_calls`].
    Compact {
        /// Tool lines, a blank line, then the call instructions.
        prompt: String,
    },
    /// Do not compact: send `tools` natively, unchanged (`compacted: false`).
    Bypass {
        /// The input tools, untouched.
        tools: Vec<ToolDef>,
        /// Why compaction was declined.
        reason: BypassReason,
    },
}

impl CompactTools {
    /// `true` for [`CompactTools::Compact`].
    pub fn compacted(&self) -> bool {
        matches!(self, Self::Compact { .. })
    }

    /// The compact prompt, if compacted.
    pub fn prompt(&self) -> Option<&str> {
        match self {
            Self::Compact { prompt } => Some(prompt),
            Self::Bypass { .. } => None,
        }
    }

    /// The bypass reason, if bypassed.
    pub fn bypass_reason(&self) -> Option<&BypassReason> {
        match self {
            Self::Compact { .. } => None,
            Self::Bypass { reason, .. } => Some(reason),
        }
    }
}

/// Why a tool list was sent natively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BypassReason {
    /// The tool list is empty; there is nothing to compact.
    NoTools,
    /// `tool_choice` was something other than absent or `"auto"` (JSON shown verbatim).
    ToolChoice(String),
    /// A tool's schema uses something outside the supported subset.
    UnsupportedSchema {
        /// Tool name.
        tool: String,
        /// JSON path inside `parameters`, e.g. `$.attendees[]`.
        path: String,
        /// What was found there.
        detail: String,
    },
    /// The compact prompt would not be smaller than the native tool JSON. Never worse than
    /// baseline: the native list is sent instead.
    NotSmaller {
        /// Compact prompt size in bytes.
        compact_bytes: usize,
        /// Minified native `tools` JSON size in bytes.
        native_bytes: usize,
    },
}

impl BypassReason {
    /// Stable label for telemetry.
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::NoTools => "no_tools",
            Self::ToolChoice(_) => "tool_choice",
            Self::UnsupportedSchema { .. } => "unsupported_schema",
            Self::NotSmaller { .. } => "not_smaller",
        }
    }
}

impl fmt::Display for BypassReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoTools => f.write_str("no tools"),
            Self::ToolChoice(choice) => write!(f, "tool_choice {choice} requires native tools"),
            Self::UnsupportedSchema { tool, path, detail } => {
                write!(f, "unsupported schema in {tool} at {path}: {detail}")
            }
            Self::NotSmaller {
                compact_bytes,
                native_bytes,
            } => write!(
                f,
                "compact {compact_bytes} B is not smaller than native {native_bytes} B"
            ),
        }
    }
}

/// Whether `tool_choice` permits compaction: absent or `"auto"` only. `"none"`, `"required"`
/// and a named function all need the provider's native enforcement, so they bypass; so does any
/// value this crate does not recognise.
pub fn tool_choice_allows_compact(tool_choice: Option<&Value>) -> bool {
    match tool_choice {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s == "auto",
        Some(_) => false,
    }
}

/// Compact a tool list with default options, or decide to bypass.
///
/// Returns [`CompactTools::Bypass`] (not an error) for an empty list, any schema outside the
/// supported subset (see `README.md`), or a prompt that would not be smaller than the native
/// JSON. Errors only on duplicate tool names, which would make decoded calls ambiguous.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    encode_tools_with(tools, &EncodeOptions::default())
}

/// [`encode_tools`], honouring the request's `tool_choice` first
/// (see [`tool_choice_allows_compact`]).
pub fn encode_tools_with_choice(
    tools: &[ToolDef],
    tool_choice: Option<&Value>,
) -> Result<CompactTools> {
    encode_tools_with(
        tools,
        &EncodeOptions {
            tool_choice,
            ..Default::default()
        },
    )
}

/// [`encode_tools`] with explicit [`EncodeOptions`].
pub fn encode_tools_with(tools: &[ToolDef], opts: &EncodeOptions<'_>) -> Result<CompactTools> {
    let mut seen = BTreeSet::new();
    for tool in tools {
        if !seen.insert(tool.name.as_str()) {
            return Err(Error::InvalidTools(format!(
                "duplicate tool name '{}'",
                tool.name
            )));
        }
    }
    let bypass = |reason| {
        Ok(CompactTools::Bypass {
            tools: tools.to_vec(),
            reason,
        })
    };
    if !tool_choice_allows_compact(opts.tool_choice) {
        let shown = opts.tool_choice.map_or_else(String::new, Value::to_string);
        return bypass(BypassReason::ToolChoice(shown));
    }
    if tools.is_empty() {
        return bypass(BypassReason::NoTools);
    }

    let mut lines = Vec::with_capacity(tools.len());
    let mut all_params = Vec::with_capacity(tools.len());
    for tool in tools {
        if !is_name(&tool.name) {
            return bypass(BypassReason::UnsupportedSchema {
                tool: tool.name.clone(),
                path: "name".into(),
                detail: "tool name outside [A-Za-z0-9_-]".into(),
            });
        }
        let params = match params_from_schema(tool.parameters.as_ref()) {
            Ok(params) => params,
            Err(u) => {
                return bypass(BypassReason::UnsupportedSchema {
                    tool: tool.name.clone(),
                    path: u.path,
                    detail: u.detail,
                });
            }
        };
        lines.push(signature::render(
            &tool.name,
            tool.description.as_deref(),
            &params,
            opts.instructions == Instructions::Balanced,
        ));
        all_params.push((tool.name.as_str(), params));
    }
    let mut prompt = lines.join("\n");
    prompt.push_str(TRAILER_SEPARATOR);
    prompt.push_str(&trailer(&all_params, opts.instructions));

    let native_bytes = Value::Array(tools.iter().map(ToolDef::to_openai).collect())
        .to_string()
        .len();
    if prompt.len() >= native_bytes {
        return bypass(BypassReason::NotSmaller {
            compact_bytes: prompt.len(),
            native_bytes,
        });
    }
    Ok(CompactTools::Compact { prompt })
}

/// Everything after the blank line: the call instruction, the notes line, and (for
/// [`Instructions::Example`]) one sample call. A pure function of the tools, so
/// [`decode_tools`] can check a prompt's trailer by rebuilding it.
fn trailer(tools: &[(&str, Obj)], instructions: Instructions) -> String {
    let (call, notes, datetime_note) = match instructions {
        Instructions::Terse => (
            TERSE_CALL_INSTRUCTION,
            Some(TERSE_NOTES),
            TERSE_DATETIME_NOTE,
        ),
        // One line: the call format and the notes are merged.
        Instructions::Balanced => (BALANCED_INSTRUCTION, None, BALANCED_DATETIME_NOTE),
        Instructions::Minimal | Instructions::Example => {
            (CALL_INSTRUCTION, Some(NOTES), DATETIME_NOTE)
        }
    };
    let mut out = String::from(call);
    if let Some(notes) = notes {
        out.push('\n');
        out.push_str(notes);
    }
    if tools.iter().any(|(_, p)| obj_has_datetime(p)) {
        out.push_str(datetime_note);
    }
    if let (Instructions::Example, Some((name, params))) = (instructions, tools.first()) {
        out.push('\n');
        out.push_str(EXAMPLE_PREFIX);
        out.push_str(&render_call(&ToolCall {
            name: (*name).to_string(),
            arguments: sample_object(params),
        }));
    }
    out
}

fn obj_has_datetime(obj: &Obj) -> bool {
    obj.fields.iter().any(|f| node_has_datetime(&f.node))
}

fn node_has_datetime(node: &Node) -> bool {
    match &node.ty {
        Ty::DateTime => true,
        Ty::Array(inner) => node_has_datetime(inner),
        Ty::Object(o) => obj_has_datetime(o),
        _ => false,
    }
}

/// Placeholder arguments for the sample call: required fields only, fixed values per type.
fn sample_object(obj: &Obj) -> Map<String, Value> {
    obj.fields
        .iter()
        .filter(|f| f.required)
        .map(|f| (f.name.clone(), sample_value(&f.node)))
        .collect()
}

fn sample_value(node: &Node) -> Value {
    match &node.ty {
        Ty::Str(_) => Value::String("text".into()),
        Ty::Int => Value::from(1),
        Ty::Num => Value::from(1.5),
        Ty::Bool => Value::Bool(true),
        Ty::DateTime => Value::String("2026-01-01T09:00:00Z".into()),
        Ty::Date => Value::String("2026-01-01".into()),
        Ty::Enum(values) => values.first().cloned().map_or(Value::Null, Value::String),
        Ty::Array(inner) => Value::Array(vec![sample_value(inner)]),
        Ty::Object(o) => Value::Object(sample_object(o)),
    }
}

/// Recover the tool list from [`encode_tools`] output.
///
/// For [`CompactTools::Bypass`] this returns the carried tools unchanged. For
/// [`CompactTools::Compact`] it parses the prompt; `decode_tools(&encode_tools(x)?)? == x` for
/// every compacted `x`, up to the normalizations in `README.md`.
pub fn decode_tools(c: &CompactTools) -> Result<Vec<ToolDef>> {
    let prompt = match c {
        CompactTools::Bypass { tools, .. } => return Ok(tools.clone()),
        CompactTools::Compact { prompt } => prompt,
    };
    // Tool lines never contain a raw newline, so the first blank line ends them.
    let (body, rest) = prompt
        .split_once(TRAILER_SEPARATOR)
        .ok_or_else(|| Error::MalformedCompact("missing call instructions".into()))?;
    let mut sigs: Vec<signature::Signature> = Vec::new();
    for (i, line) in body.split('\n').enumerate() {
        let sig = signature::parse(line)
            .map_err(|e| Error::MalformedCompact(format!("line {}: {e}", i + 1)))?;
        if sigs.iter().any(|s| s.name == sig.name) {
            return Err(Error::MalformedCompact(format!(
                "line {}: duplicate tool '{}'",
                i + 1,
                sig.name
            )));
        }
        sigs.push(sig);
    }
    let params: Vec<(&str, Obj)> = sigs
        .iter()
        .map(|s| (s.name.as_str(), s.params.clone()))
        .collect();
    let known = Instructions::ALL
        .iter()
        .any(|&i| trailer(&params, i) == rest);
    if !known {
        return Err(Error::MalformedCompact(
            "unrecognised call instructions".into(),
        ));
    }
    Ok(sigs
        .into_iter()
        .map(|sig| ToolDef {
            parameters: Some(params_to_schema(&sig.params)),
            name: sig.name,
            description: sig.desc,
        })
        .collect())
}
