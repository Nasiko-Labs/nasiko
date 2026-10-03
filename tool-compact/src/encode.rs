//! Tools → compact definition lines.

use crate::error::{CompactError, Result};
use crate::schema::{self, Fields, Node, Nullable, Tool, Ty};
use crate::text;
use crate::types::{CompactTools, ToolCall, ToolDef};

/// First line of [`CompactTools::prompt`]. `?` is the only sigil a model cannot be expected to
/// guess; `[T]`, `{..}` and `a|b` read the way they do in every typed language.
pub(crate) const HEADER: &str = "Tools (? = optional):";

/// Kept short on purpose: this is paid on every request, against the tokens the definitions save.
pub(crate) const INSTRUCTIONS: &str =
    "To call a tool, emit <<call name {json args}>>, one per call. Otherwise reply in plain text.";

pub(crate) const CALL_OPEN: &str = "<<call";
pub(crate) const CALL_CLOSE: &str = ">>";

/// Write `tools` in the compact form.
///
/// Fails with [`CompactError::Unsupported`] if any tool uses a schema feature the form cannot
/// carry; the caller then sends the native definitions for the whole request.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    if tools.is_empty() {
        return Err(CompactError::Unsupported {
            tool: String::new(),
            reason: "no tools to encode".into(),
        });
    }
    let lines: Vec<String> = schema::compile(tools)?.iter().map(tool_line).collect();
    Ok(CompactTools {
        definitions: lines.join("\n"),
        instructions: INSTRUCTIONS.to_string(),
    })
}

/// Write calls the way the model is asked to, one per line.
///
/// For the caller's use when replaying earlier calls into a compacted conversation. The arguments
/// are checked to be a JSON object and then written as given.
pub fn render_calls(calls: &[ToolCall]) -> Result<String> {
    let mut lines = Vec::with_capacity(calls.len());
    for call in calls {
        if !text::is_name(&call.name) {
            return Err(CompactError::Malformed(format!(
                "tool name '{}' cannot be written in a call",
                call.name
            )));
        }
        match serde_json::from_str::<serde_json::Value>(&call.arguments) {
            Ok(serde_json::Value::Object(_)) => {}
            _ => {
                return Err(CompactError::InvalidArguments {
                    tool: call.name.clone(),
                    reason: "arguments are not a JSON object".into(),
                });
            }
        }
        lines.push(format!(
            "{CALL_OPEN} {} {}{CALL_CLOSE}",
            call.name,
            call.arguments.trim()
        ));
    }
    Ok(lines.join("\n"))
}

fn tool_line(tool: &Tool) -> String {
    let mut out = tool.name.clone();
    if let Some(params) = &tool.params {
        out.push('(');
        out.push_str(&fields(params));
        out.push(')');
        if params.closed {
            out.push('!');
        }
        if let Some(title) = &tool.params_title {
            out.push('@');
            out.push_str(&text::quote(title));
        }
    }
    if let Some(description) = &tool.description {
        out.push_str(" - ");
        if reads_raw(description) {
            out.push_str(description);
        } else {
            out.push_str(&text::quote(description));
        }
    }
    out
}

/// Whether a tool description can run bare to the end of its line and still parse back exactly.
fn reads_raw(description: &str) -> bool {
    !description.is_empty()
        && !description.starts_with('\'')
        && description.trim() == description
        && !description.chars().any(char::is_control)
}

fn fields(fields: &Fields) -> String {
    let parts: Vec<String> = fields
        .fields
        .iter()
        .map(|field| {
            let key = if text::is_ident(&field.key) {
                field.key.clone()
            } else {
                text::quote(&field.key)
            };
            let optional = if field.required { "" } else { "?" };
            format!("{key}{optional}:{}", node(&field.node, Some(&field.key)))
        })
        .collect();
    parts.join(", ")
}

/// `key` is the field the node belongs to, if any; it lets a title that merely restates the key
/// be written as a bare `@`.
fn node(node: &Node, key: Option<&str>) -> String {
    let mut out = ty(&node.ty);
    if let Some(range) = &node.range {
        let bound = |b: &Option<serde_json::Number>| b.as_ref().map(ToString::to_string);
        out.push_str(&format!(
            "({}..{})",
            bound(&range.min).unwrap_or_default(),
            bound(&range.max).unwrap_or_default()
        ));
    }
    out.push_str(match node.nullable {
        Nullable::No => "",
        Nullable::TypeList => "|null",
        Nullable::AnyOf => "|null~",
        Nullable::AnyOfNullFirst => "|null~~",
    });
    match &node.default {
        Some(serde_json::Value::String(s)) => out.push_str(&format!("={}", text::quote(s))),
        Some(scalar) => out.push_str(&format!("={scalar}")),
        None => {}
    }
    if let Some(title) = &node.title {
        out.push('@');
        if key.map(schema::derived_title).as_ref() != Some(title) {
            out.push_str(&text::quote(title));
        }
    }
    if let Some(description) = &node.description {
        out.push(' ');
        out.push_str(&text::quote(description));
    }
    out
}

fn ty(ty: &Ty) -> String {
    match ty {
        Ty::Str { format: None } => "str".into(),
        Ty::Str { format: Some(f) } if f == "date-time" => "datetime".into(),
        Ty::Str { format: Some(f) } => format!("str<{f}>"),
        Ty::Int => "int".into(),
        Ty::Num => "num".into(),
        Ty::Bool => "bool".into(),
        Ty::AnyObject => "obj".into(),
        Ty::StrEnum(values) => {
            // A lone bare word would read as a type name, so a one-value enum is always quoted.
            let bare =
                |v: &str| values.len() > 1 && text::is_ident(v) && !text::KEYWORDS.contains(&v);
            let parts: Vec<String> = values
                .iter()
                .map(|v| if bare(v) { v.clone() } else { text::quote(v) })
                .collect();
            parts.join("|")
        }
        Ty::IntEnum(values) => {
            let parts: Vec<String> = values.iter().map(i64::to_string).collect();
            parts.join("|")
        }
        // Wrapped, so a whole-number choice is not read back as an integer enum.
        Ty::NumEnum(values) => {
            let parts: Vec<String> = values.iter().map(ToString::to_string).collect();
            format!("num({})", parts.join("|"))
        }
        Ty::BoolEnum(values) => {
            let parts: Vec<String> = values.iter().map(bool::to_string).collect();
            format!("bool({})", parts.join("|"))
        }
        Ty::Array(items) => format!("[{}]", node(items, None)),
        Ty::Object(inner) => {
            let closed = if inner.closed { "!" } else { "" };
            format!("{{{}}}{closed}", fields(inner))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar() -> ToolDef {
        ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event in the user's calendar.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string", "format": "date-time"},
                    "duration_min": {"type": "integer"},
                    "attendees": {"type": "array", "items": {"type": "string"}},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        }
    }

    #[test]
    fn a_tool_is_one_line_with_every_piece_of_schema_meaning() {
        let compact = encode_tools(&[calendar()]).unwrap();
        let line = &compact.definitions;
        assert!(!line.contains('\n'));
        assert!(line.starts_with("create_calendar_event("));
        assert!(line.ends_with(") - Create an event in the user's calendar."));
        for piece in [
            "title:str",
            "start:datetime",
            "duration_min?:int",
            "attendees?:[str]",
            "visibility?:public|private",
        ] {
            assert!(line.contains(piece), "missing {piece} in {line}");
        }
    }

    #[test]
    fn prompt_carries_header_definitions_and_call_format() {
        let compact = encode_tools(&[calendar()]).unwrap();
        let prompt = compact.prompt();
        assert!(prompt.starts_with(HEADER));
        assert!(prompt.contains(&compact.definitions));
        assert!(prompt.ends_with(INSTRUCTIONS));
        assert!(INSTRUCTIONS.contains("<<call name {json args}>>"));
    }

    #[test]
    fn descriptions_are_kept_and_never_use_double_quotes() {
        let tool = ToolDef {
            name: "t".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {"to": {"type": "string", "description": "Recipient's email"}}
            })),
        };
        let line = encode_tools(&[tool]).unwrap().definitions;
        assert_eq!(line, "t(to?:str 'Recipient\\'s email')");
    }

    #[test]
    fn awkward_names_and_values_are_quoted() {
        let tool = ToolDef {
            name: "t".into(),
            description: Some(" padded\nand split ".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "a b": {"type": "string", "enum": ["only"]},
                    "kind": {"type": "string", "enum": ["str", "two words", "42", "ok"]},
                    "level": {"type": "integer", "enum": [-1, 2]}
                },
                "additionalProperties": false
            })),
        };
        let line = encode_tools(&[tool]).unwrap().definitions;
        assert!(!line.contains('\n'));
        assert!(line.contains("'a b'?:'only'"));
        assert!(line.contains("kind?:'str'|'two words'|'42'|ok"));
        assert!(line.contains("level?:-1|2"));
        assert!(line.contains(")! - ' padded\\nand split '"));
    }

    #[test]
    fn a_tool_without_parameters_has_no_parens_and_an_empty_one_does() {
        let bare = ToolDef {
            name: "ping".into(),
            description: None,
            parameters: None,
        };
        let empty = ToolDef {
            parameters: Some(json!({"type": "object", "properties": {}})),
            ..bare.clone()
        };
        assert_eq!(encode_tools(&[bare]).unwrap().definitions, "ping");
        assert_eq!(encode_tools(&[empty]).unwrap().definitions, "ping()");
    }

    #[test]
    fn an_unsupported_tool_stops_the_whole_encoding() {
        let odd = ToolDef {
            name: "odd".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {"p": {"type": "string", "pattern": "^a"}}
            })),
        };
        let err = encode_tools(&[calendar(), odd]).unwrap_err();
        assert_eq!(err.as_label(), "unsupported_schema");
        assert!(encode_tools(&[]).is_err());
    }

    #[test]
    fn encoding_is_deterministic() {
        let tools = [calendar()];
        assert_eq!(encode_tools(&tools).unwrap(), encode_tools(&tools).unwrap());
    }

    #[test]
    fn render_calls_writes_the_call_grammar_and_refuses_non_objects() {
        let call = ToolCall {
            name: "send_email".into(),
            arguments: r#"{"subject":"a >> b"}"#.into(),
        };
        assert_eq!(
            render_calls(&[call.clone(), call]).unwrap(),
            "<<call send_email {\"subject\":\"a >> b\"}>>\n<<call send_email {\"subject\":\"a >> b\"}>>"
        );
        for arguments in ["[1]", "not json", "\"s\"", ""] {
            let bad = ToolCall {
                name: "t".into(),
                arguments: arguments.into(),
            };
            assert!(render_calls(&[bad]).is_err(), "rendered {arguments}");
        }
        let bad_name = ToolCall {
            name: "a b".into(),
            arguments: "{}".into(),
        };
        assert!(render_calls(&[bad_name]).is_err());
    }
}
