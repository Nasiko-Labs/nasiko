//! CompTrust compact tool schemas — compact when safe, preserve when uncertain, decode
//! deterministically, reject invalid calls.
//!
//! ```text
//! tool schemas ──encode_tools──▶ compact signatures + call-format instructions ──▶ LLM
//! LLM text ──StreamDecoder / decode_calls──▶ validated ToolCalls   (or an explicit Error)
//! ```
//!
//! Pure library: no IO, no env reads, no provider code, no dependency on the router. It defines
//! its own [`ToolDef`] / [`ToolCall`]; the router converts at the seam and assigns call ids.
//!
//! # Grammar
//!
//! One signature per line, one tool per signature:
//!
//! ```text
//! tool   = name "(" [ param { "," param } ] ")" [ " - " description ]
//! param  = ident [ "?" ] ":" type [ " " json-string ]      ; "?" = optional, json-string = note
//! type   = "str" | "int" | "num" | "bool" | "datetime"
//!        | "[" type "]"                                    ; array
//!        | "{" [ param { "," param } ] "}"                 ; nested object
//!        | value "|" value { "|" value }                   ; string enum, declared order
//! name   = 1*64 ( ALPHA | DIGIT | "_" | "-" )
//! ident  = ( ALPHA | "_" ) *( ALPHA | DIGIT | "_" )
//! value  = 1*( ALPHA | DIGIT | one of "_.+/@#-" )          ; not a type keyword
//!
//! call   = "<<call" WS name WS json-object [ WS ] ">>"     ; WS = space, tab, CR or LF
//! ```
//!
//! Text outside a `call` is ignored. `<<call` followed by anything but whitespace is prose. A
//! `call` that starts but is malformed, unterminated or invalid is an [`Error`], never a call.
//! The end of the JSON object is found by brace matching that skips strings and escapes, so
//! `>>` inside a string argument never ends a call.
//!
//! # Supported schema subset
//!
//! Per property: `string` (optionally `format: date-time`), `integer`, `number`, `boolean`,
//! string `enum` (≥2 distinct values from the token alphabet above), `array` with a single
//! `items` schema, nested `object` with `properties`/`required`; plus `description`.
//! `additionalProperties: false` and `required: []` are accepted.
//!
//! **Anything else bypasses compaction** for that tool: other keywords (`minLength`, `pattern`,
//! `default`, `$ref`, `oneOf`, `title`, …), other `format`s, union types, free-form
//! objects/arrays, names/descriptions that cannot be written on one line. A bypassed tool is
//! returned unchanged in [`CompactTools::native`] to be sent as an ordinary tool, and a compact
//! call to it is rejected.
//!
//! # Fail-closed validation
//!
//! Every call is checked against the *original* schema: unknown tool, unknown or missing
//! argument, wrong type (`30.0` is not an integer), enum miss (exact match), bad `date-time`,
//! malformed or duplicate-key JSON. Nothing is renamed, defaulted, coerced or repaired. Decoding
//! is stricter than JSON Schema's default in one way: undeclared arguments are rejected.

mod decode;
mod schema;

use std::collections::HashSet;

use serde_json::Value;

pub use decode::StreamDecoder;

/// A tool definition: the fields of an OpenAI function tool that matter here.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema for the arguments.
    pub parameters: Option<Value>,
}

/// A decoded call. `arguments` is a JSON **string** (OpenAI's contract), canonicalised from the
/// validated value; the router assigns the call `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("duplicate tool name `{0}`")]
    DuplicateTool(String),
    #[error("unknown tool `{0}`")]
    UnknownTool(String),
    #[error("invalid arguments for `{tool}`: {reason}")]
    InvalidArguments { tool: String, reason: String },
    #[error("malformed tool call: {0}")]
    MalformedCall(String),
    #[error("tool `{0}` has no compact form and must be called natively")]
    UnsupportedSchema(String),
    #[error("invalid compact signatures: {0}")]
    InvalidSignatures(String),
}

impl Error {
    /// Stable machine-readable code (`unknown_tool` and `invalid_arguments` are the eval contract).
    pub fn code(&self) -> &'static str {
        match self {
            Self::DuplicateTool(_) => "duplicate_tool",
            Self::UnknownTool(_) => "unknown_tool",
            Self::InvalidArguments { .. } => "invalid_arguments",
            Self::MalformedCall(_) => "malformed_call",
            Self::UnsupportedSchema(_) => "unsupported_schema",
            Self::InvalidSignatures(_) => "invalid_signatures",
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Why a tool was left in its original form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bypass {
    pub name: String,
    pub reason: String,
}

/// Output of [`encode_tools`].
#[derive(Debug, Clone, PartialEq)]
pub struct CompactTools {
    /// One compact signature per line for every tool that could be compacted.
    pub signatures: String,
    /// Tools with no exact compact form, unchanged: send them as ordinary tools.
    pub native: Vec<ToolDef>,
    pub bypassed: Vec<Bypass>,
}

const INSTRUCTIONS: &str = "Call a tool: <<call name {json args}>> (several allowed). ?=optional, datetime=RFC 3339 with offset. Only listed tools and args; if none fits, reply in plain text.";

impl CompactTools {
    /// Text to inject into the prompt (empty when nothing was compacted): the signatures plus
    /// the call-format instructions.
    pub fn prompt(&self) -> String {
        if self.signatures.is_empty() {
            return String::new();
        }
        format!("{}\n{INSTRUCTIONS}", self.signatures)
    }

    /// Whether at least one tool was compacted.
    pub fn is_compacted(&self) -> bool {
        !self.signatures.is_empty()
    }
}

pub(crate) fn ensure_unique(tools: &[ToolDef]) -> Result<()> {
    let mut seen = HashSet::new();
    match tools.iter().find(|t| !seen.insert(t.name.as_str())) {
        Some(t) => Err(Error::DuplicateTool(t.name.clone())),
        None => Ok(()),
    }
}

/// Encode tools compactly. Tools that cannot be represented exactly are bypassed, not degraded.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    ensure_unique(tools)?;
    let (mut lines, mut native, mut bypassed) = (Vec::new(), Vec::new(), Vec::new());
    for t in tools {
        match schema::compile(t) {
            Ok(sig) => lines.push(sig.render()),
            Err(reason) => {
                native.push(t.clone());
                bypassed.push(Bypass {
                    name: t.name.clone(),
                    reason,
                });
            }
        }
    }
    Ok(CompactTools {
        signatures: lines.join("\n"),
        native,
        bypassed,
    })
}

/// Rebuild tool definitions from compact signatures (plus the untouched native tools), so a
/// caller can check that schema information survived. Compacted tools come first, in order.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    let mut tools = compact
        .signatures
        .lines()
        .map(|l| {
            schema::parse_line(l)
                .map(schema::Sig::into_tool)
                .map_err(Error::InvalidSignatures)
        })
        .collect::<Result<Vec<_>>>()?;
    tools.extend(compact.native.iter().cloned());
    Ok(tools)
}

/// Decode every call in `text`, validating each against `tools`. Any invalid call fails the
/// whole text; plain text and no calls at all are fine.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut d = StreamDecoder::new(tools)?;
    let calls = d.push(text)?;
    d.finish()?;
    Ok(calls)
}

/// Write a call in the compact call syntax (useful for tests, evals and history rewriting).
pub fn render_call(name: &str, args: &Value) -> String {
    format!("<<call {name} {args}>>")
}
