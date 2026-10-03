//! Typed tree → definition text, plus the encoder's two safety checks (self round trip and
//! never-grows).
//!
//! Definition grammar (full EBNF in `README.md`):
//!
//! ```text
//! ## create_calendar_event: Create an event in the user's calendar.
//! title: str # Event title
//! attendees?: [str] # Attendee emails
//! visibility?: public|private
//! ```

use serde_json::{Value, json};

use crate::error::EncodeError;
use crate::parse::parse_definitions;
use crate::schema::{Prop, Tool, Ty, build_tool, is_bare_enum_value};
use crate::types::{CompactTools, EncodeOptions, ToolDef};

/// The call-format instructions placed before the definitions. Kept deliberately short: on
/// small tool sets this fixed cost dominates the token budget (≈ 20 tokens, `o200k_base`).
pub const INSTRUCTIONS: &str =
    "To use a tool write <<call NAME {JSON args}>> per call (?=optional). Else answer normally.";

/// Encode with default options ([`crate::DescriptionPolicy::Verbatim`]).
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, EncodeError> {
    encode_tools_with(tools, &EncodeOptions::default())
}

/// Encode `tools` into compact text, or refuse (the caller then sends the native tools).
///
/// Refuses when any tool is outside the supported subset, when two tools share a name, when the
/// rendered text does not parse back to the same schemas, or when the result would not be
/// smaller than the native JSON.
pub fn encode_tools_with(
    tools: &[ToolDef],
    opts: &EncodeOptions,
) -> Result<CompactTools, EncodeError> {
    if tools.is_empty() {
        return Err(EncodeError::NoTools);
    }
    let mut built: Vec<Tool> = Vec::with_capacity(tools.len());
    for def in tools {
        let tool = build_tool(def, opts.descriptions)?;
        if built.iter().any(|t| t.name == tool.name) {
            return Err(EncodeError::Unsupported {
                tool: tool.name,
                path: "/".into(),
                keyword: "duplicate name".into(),
            });
        }
        built.push(tool);
    }

    let definitions = render_definitions(&built);

    // Self-check: what a reader of the text alone recovers must equal what we meant to encode.
    let reparsed = parse_definitions(&definitions).map_err(|_| EncodeError::RoundTrip {
        tool: built.first().map(|t| t.name.clone()).unwrap_or_default(),
    })?;
    if reparsed.len() != built.len() {
        return Err(EncodeError::RoundTrip {
            tool: built.first().map(|t| t.name.clone()).unwrap_or_default(),
        });
    }
    for (a, b) in built.iter().zip(&reparsed) {
        if a != b {
            return Err(EncodeError::RoundTrip {
                tool: a.name.clone(),
            });
        }
    }

    let compact = CompactTools {
        instructions: INSTRUCTIONS.to_string(),
        definitions,
    };
    if compact.system_text().len() >= native_len(tools) {
        return Err(EncodeError::NotSmaller);
    }
    Ok(compact)
}

/// Byte length of the native OpenAI `tools` array these definitions replace — the never-grows
/// yardstick (bytes are the library-side proxy; the eval reports tokens).
pub(crate) fn native_len(tools: &[ToolDef]) -> usize {
    let native: Vec<Value> = tools
        .iter()
        .map(|t| {
            let mut function = json!({ "name": t.name });
            if let Some(d) = &t.description {
                function["description"] = json!(d);
            }
            if let Some(p) = &t.parameters {
                function["parameters"] = p.clone();
            }
            json!({ "type": "function", "function": function })
        })
        .collect();
    serde_json::to_string(&native).map(|s| s.len()).unwrap_or(0)
}

pub(crate) fn render_definitions(tools: &[Tool]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for tool in tools {
        match &tool.desc {
            Some(d) => lines.push(format!("## {}: {d}", tool.name)),
            None => lines.push(format!("## {}", tool.name)),
        }
        render_props(&tool.params, 0, &mut lines);
    }
    lines.join("\n")
}

fn render_props(props: &[Prop], depth: usize, lines: &mut Vec<String>) {
    for p in props {
        let mut line = "  ".repeat(depth);
        line.push_str(&p.key);
        if !p.required {
            line.push('?');
        }
        line.push_str(": ");
        line.push_str(&type_text(&p.ty));
        if p.min.is_some() || p.max.is_some() {
            line.push(' ');
            if let Some(n) = &p.min {
                line.push_str(&n.to_string());
            }
            line.push_str("..");
            if let Some(n) = &p.max {
                line.push_str(&n.to_string());
            }
        }
        if let Some(d) = &p.default {
            line.push_str(" = ");
            line.push_str(&d.to_string());
        }
        if let Some(d) = &p.desc {
            line.push_str(" # ");
            line.push_str(d);
        }
        lines.push(line);
        if let Some(children) = innermost_object(&p.ty) {
            render_props(children, depth + 1, lines);
        }
    }
}

/// The properties of the object a type ends in (`obj`, `[obj]`, `[[obj]]`), whose child lines
/// follow the property's own line.
pub(crate) fn innermost_object(ty: &Ty) -> Option<&Vec<Prop>> {
    match ty {
        Ty::Obj(props) => Some(props),
        Ty::Array(inner) => innermost_object(inner),
        _ => None,
    }
}

fn type_text(ty: &Ty) -> String {
    match ty {
        Ty::Str(None) => "str".into(),
        Ty::Str(Some(f)) => f.keyword().into(),
        Ty::Int => "int".into(),
        Ty::Num => "num".into(),
        Ty::Bool => "bool".into(),
        Ty::Null => "null".into(),
        Ty::Obj(_) => "obj".into(),
        Ty::Array(inner) => format!("[{}]", type_text(inner)),
        Ty::Enum(values) => {
            let bare: Option<Vec<&str>> = values
                .iter()
                .map(|v| v.as_str().filter(|s| is_bare_enum_value(s)))
                .collect();
            match bare {
                Some(words) => words.join("|"),
                None => format!("enum{}", Value::Array(values.clone())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DescriptionPolicy, decode_tools, normalize};
    use serde_json::json;

    fn calendar() -> ToolDef {
        ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event in the user's calendar.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Event title"},
                    "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                    "duration_min": {"type": "integer", "description": "Duration in minutes"},
                    "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        }
    }

    fn email() -> ToolDef {
        ToolDef {
            name: "send_email".into(),
            description: Some("Send an email from the user's account.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string"}, "description": "Recipient emails"},
                    "subject": {"type": "string", "description": "Subject line"},
                    "body": {"type": "string", "description": "Plain-text body"},
                    "cc": {"type": "array", "items": {"type": "string"}, "description": "CC emails"}
                },
                "required": ["to", "subject", "body"]
            })),
        }
    }

    #[test]
    fn golden_public_tools() {
        let c = encode_tools(&[calendar(), email()]).unwrap();
        assert_eq!(
            c.definitions,
            "## create_calendar_event: Create an event in the user's calendar.\n\
             title: str # Event title\n\
             start: datetime # Start time, ISO 8601\n\
             attendees?: [str] # Attendee emails\n\
             duration_min?: int # Duration in minutes\n\
             visibility?: public|private\n\
             ## send_email: Send an email from the user's account.\n\
             to: [str] # Recipient emails\n\
             subject: str # Subject line\n\
             body: str # Plain-text body\n\
             cc?: [str] # CC emails"
        );
        assert_eq!(c.instructions, INSTRUCTIONS);
        assert!(
            !c.system_text().contains('"'),
            "definitions must be quote-free"
        );
    }

    #[test]
    fn deterministic_bytes() {
        let a = encode_tools(&[calendar(), email()]).unwrap();
        let b = encode_tools(&[calendar(), email()]).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn drop_redundant_only_drops_redundant() {
        let opts = EncodeOptions {
            descriptions: DescriptionPolicy::DropRedundant,
        };
        let c = encode_tools_with(&[calendar()], &opts).unwrap();
        assert!(c.definitions.contains("title: str\n"), "{}", c.definitions);
        assert!(c.definitions.contains("# Attendee emails"));
        assert!(c.definitions.contains("# Duration in minutes"));
        let back = decode_tools(&c).unwrap();
        assert_eq!(
            back,
            vec![normalize(&calendar(), DescriptionPolicy::DropRedundant).unwrap()]
        );
    }

    #[test]
    fn nested_objects_ranges_defaults_enums_round_trip() {
        let t = ToolDef {
            name: "plan.trip-v2".into(),
            description: Some("Plan\n   a   trip.".into()),
            parameters: Some(json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "legs": {"type": "array", "items": {"type": "object", "properties": {
                        "from": {"type": "string"},
                        "to": {"type": "string"},
                        "day": {"type": "string", "format": "date"},
                        "seat": {"type": "object", "properties": {"row": {"type": "integer"}}}
                    }, "required": ["from", "to"]}},
                    "budget": {"type": "number", "minimum": 0, "maximum": 99.5, "description": "Max spend # in USD"},
                    "travellers": {"type": "integer", "minimum": 1, "default": 1},
                    "class": {"type": "string", "enum": ["economy", "business", "first class"]},
                    "stars": {"enum": [3, 4, 5]},
                    "kind": {"type": "string", "enum": ["str", "obj"]},
                    "note": {"type": "string", "default": "a # b \" c"},
                    "flex": {"type": "boolean"},
                    "nothing": {"type": "null"},
                    "id": {"type": "string", "format": "uuid"},
                    "grid": {"type": "array", "items": {"type": "array", "items": {"type": "integer"}}}
                },
                "required": ["legs"]
            })),
        };
        let c = encode_tools(std::slice::from_ref(&t)).unwrap();
        let back = decode_tools(&c).unwrap();
        assert_eq!(
            back,
            vec![normalize(&t, DescriptionPolicy::Verbatim).unwrap()]
        );
        assert!(
            c.definitions
                .contains("class?: enum[\"economy\",\"business\",\"first class\"]")
        );
        assert!(c.definitions.contains("stars?: enum[3,4,5]"));
        assert!(c.definitions.contains("kind?: enum[\"str\",\"obj\"]"));
        assert!(
            c.definitions
                .contains("budget?: num 0..99.5 # Max spend # in USD")
        );
        assert!(c.definitions.contains("travellers?: int 1.. = 1"));
        assert!(c.definitions.contains(
            "legs: [obj]\n  from: str\n  to: str\n  day?: date\n  seat?: obj\n    row?: int"
        ));
        assert!(c.definitions.starts_with("## plan.trip-v2: Plan a trip.\n"));
    }

    #[test]
    fn no_parameter_tools_and_empty_required() {
        let tools = vec![
            ToolDef {
                name: "ping".into(),
                description: Some("Check the service is alive and responding to requests.".into()),
                parameters: None,
            },
            ToolDef {
                name: "list_items".into(),
                description: None,
                parameters: Some(json!({"type": "object", "properties": {}, "required": []})),
            },
            calendar(),
        ];
        let c = encode_tools(&tools).unwrap();
        let back = decode_tools(&c).unwrap();
        for (orig, got) in tools.iter().zip(&back) {
            assert_eq!(got, &normalize(orig, DescriptionPolicy::Verbatim).unwrap());
        }
    }

    #[test]
    fn unsupported_keywords_bypass_with_path() {
        let cases = [
            (
                json!({"type": "object", "properties": {"a": {"anyOf": [{"type": "string"}]}}}),
                "/a",
                "anyOf",
            ),
            (
                json!({"type": "object", "properties": {"a": {"oneOf": []}}}),
                "/a",
                "oneOf",
            ),
            (
                json!({"type": "object", "properties": {"a": {"$ref": "#/x"}}}),
                "/a",
                "$ref",
            ),
            (
                json!({"type": "object", "$defs": {}, "properties": {}}),
                "/",
                "$defs",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": ["string", "null"]}}}),
                "/a",
                "type",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string", "pattern": "x"}}}),
                "/a",
                "pattern",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string", "minLength": 1}}}),
                "/a",
                "minLength",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "array", "items": {"type": "string"}, "maxItems": 3}}}),
                "/a",
                "maxItems",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string", "format": "hostname"}}}),
                "/a",
                "format",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "object"}}}),
                "/a",
                "properties",
            ),
            (
                json!({"type": "object", "properties": {"a": {}}}),
                "/a",
                "type",
            ),
            (
                json!({"type": "object", "additionalProperties": true, "properties": {}}),
                "/",
                "additionalProperties",
            ),
            (
                json!({"type": "object", "properties": {"a b": {"type": "string"}}}),
                "/a b",
                "property name",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "array"}}}),
                "/a",
                "items",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "array", "items": {"type": "string", "description": "x"}}}}),
                "/a/items",
                "description",
            ),
            (
                json!({"type": "object", "properties": {"a": {"enum": ["x", 1]}}}),
                "/a",
                "enum",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string", "default": ["x"]}}}),
                "/a",
                "default",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string", "minimum": 1}}}),
                "/a",
                "minimum",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string", "title": "A"}}}),
                "/a",
                "title",
            ),
            (
                json!({"type": "object", "properties": {}, "required": ["missing"]}),
                "/",
                "required",
            ),
            (json!({"type": "string"}), "/", "type"),
        ];
        for (params, path, keyword) in cases {
            let t = ToolDef {
                name: "t".into(),
                description: None,
                parameters: Some(params.clone()),
            };
            match encode_tools(&[t]) {
                Err(EncodeError::Unsupported {
                    path: p,
                    keyword: k,
                    ..
                }) => {
                    assert_eq!((p.as_str(), k.as_str()), (path, keyword), "{params}");
                }
                other => panic!("{params}: expected Unsupported, got {other:?}"),
            }
        }
    }

    #[test]
    fn bad_names_and_duplicates_bypass() {
        let bad = ToolDef {
            name: "has space".into(),
            description: None,
            parameters: None,
        };
        assert!(matches!(
            encode_tools(&[bad]),
            Err(EncodeError::Unsupported { .. })
        ));
        assert!(matches!(
            encode_tools(&[calendar(), calendar()]),
            Err(EncodeError::Unsupported { .. })
        ));
        assert_eq!(encode_tools(&[]), Err(EncodeError::NoTools));
    }

    #[test]
    fn never_grows() {
        let tiny = ToolDef {
            name: "ping".into(),
            description: None,
            parameters: None,
        };
        assert_eq!(encode_tools(&[tiny]), Err(EncodeError::NotSmaller));
    }
}
