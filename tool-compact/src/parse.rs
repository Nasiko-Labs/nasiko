//! Compact definition lines → tools. The inverse of [`crate::encode`].

use crate::error::{CompactError, Result};
use serde_json::{Number, Value};

use crate::schema::{self, Field, Fields, MAX_DEPTH, Node, Range, Tool, Ty};
use crate::text::{self, Cursor};
use crate::types::{CompactTools, ToolDef};

/// Read compact definitions back into tools with full JSON Schema.
///
/// Exists so schema preservation can be checked mechanically: for every supported tool list,
/// `decode_tools(&encode_tools(tools)?)` describes the same tools.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    let mut tools: Vec<Tool> = Vec::new();
    for line in compact.definitions.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        let tool = tool_line(line)
            .map_err(|reason| CompactError::Malformed(format!("{reason} in `{line}`")))?;
        if tools.iter().any(|t| t.name == tool.name) {
            return Err(CompactError::Malformed(format!(
                "tool '{}' is defined twice",
                tool.name
            )));
        }
        tools.push(tool);
    }
    Ok(tools.iter().map(schema::to_def).collect())
}

fn tool_line(line: &str) -> std::result::Result<Tool, String> {
    let mut c = Cursor::new(line);
    let name = c.take_while(text::is_name_char);
    if name.is_empty() {
        return Err("missing tool name".into());
    }
    let params = if c.eat('(') {
        let fields = fields(&mut c, ')', 0)?;
        Some(Fields {
            fields,
            closed: c.eat('!'),
        })
    } else {
        None
    };
    let description = if c.at_end() {
        None
    } else {
        for expected in [' ', '-', ' '] {
            c.expect(expected)?;
        }
        if c.peek() == Some('\'') {
            let quoted = c.quoted()?;
            if !c.at_end() {
                return Err("text after the quoted description".into());
            }
            Some(quoted)
        } else {
            let rest = c.rest();
            if rest.is_empty() {
                return Err("empty description".into());
            }
            Some(rest)
        }
    };
    Ok(Tool {
        name,
        description,
        params,
    })
}

/// Parse `key[?]:type, ...` up to and including `close`.
fn fields(c: &mut Cursor, close: char, depth: usize) -> std::result::Result<Vec<Field>, String> {
    let mut out: Vec<Field> = Vec::new();
    if c.eat(close) {
        return Ok(out);
    }
    loop {
        let key = if c.peek() == Some('\'') {
            c.quoted()?
        } else {
            let key = c.take_while(text::is_ident_char);
            if !text::is_ident(&key) {
                return Err(format!("bad param name `{key}`"));
            }
            key
        };
        let required = !c.eat('?');
        c.expect(':')?;
        let node = node(c, depth + 1)?;
        if out.iter().any(|f| f.key == key) {
            return Err(format!("param `{key}` appears twice"));
        }
        out.push(Field {
            key,
            required,
            node,
        });
        if c.eat(',') {
            c.skip_spaces();
        } else {
            c.expect(close)?;
            return Ok(out);
        }
    }
}

fn node(c: &mut Cursor, depth: usize) -> std::result::Result<Node, String> {
    if depth > MAX_DEPTH {
        return Err(format!("types nest deeper than {MAX_DEPTH} levels"));
    }
    let ty = ty(c, depth)?;
    let range = if c.peek() == Some('(') {
        Some(range(c, &ty)?)
    } else {
        None
    };
    let nullable = c.eat_null();
    let default = if c.eat('=') { Some(default(c)?) } else { None };
    let description = if c.peek() == Some(' ') && c.peek_at(1) == Some('\'') {
        c.bump();
        Some(c.quoted()?)
    } else {
        None
    };
    Ok(Node {
        ty,
        nullable,
        range,
        default,
        description,
    })
}

/// `(min..max)`, either side optional.
fn range(c: &mut Cursor, ty: &Ty) -> std::result::Result<Range, String> {
    let (_, _, counts) = schema::range_keys(ty).ok_or("this type takes no range")?;
    c.expect('(')?;
    let inner = c.take_while(|ch| ch != ')');
    c.expect(')')?;
    let (min, max) = inner.split_once("..").ok_or("range has no `..`")?;
    let bound = |s: &str| {
        if s.is_empty() {
            return Ok(None);
        }
        serde_json::from_str::<Number>(s)
            .ok()
            .filter(|n| !counts || n.is_u64())
            .map(Some)
            .ok_or_else(|| format!("bad range bound `{s}`"))
    };
    let range = Range {
        min: bound(min)?,
        max: bound(max)?,
    };
    if range.min.is_none() && range.max.is_none() {
        return Err("empty range".into());
    }
    Ok(range)
}

/// A scalar default: a quoted string, or a bare JSON number, `true`, `false` or `null`.
fn default(c: &mut Cursor) -> std::result::Result<Value, String> {
    if c.peek() == Some('\'') {
        return Ok(Value::String(c.quoted()?));
    }
    let raw = c.take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '+' | '.'));
    match serde_json::from_str::<Value>(&raw) {
        Ok(scalar @ (Value::Null | Value::Bool(_) | Value::Number(_))) => Ok(scalar),
        _ => Err(format!("bad default `{raw}`")),
    }
}

/// The `|`-separated choices inside `num(..)` and `bool(..)`.
fn wrapped<T>(
    c: &mut Cursor,
    parse: impl Fn(&str) -> Option<T>,
) -> std::result::Result<Vec<T>, String> {
    c.expect('(')?;
    let inner = c.take_while(|ch| ch != ')');
    c.expect(')')?;
    inner
        .split('|')
        .map(|s| parse(s).ok_or_else(|| format!("bad enum value `{s}`")))
        .collect()
}

fn ty(c: &mut Cursor, depth: usize) -> std::result::Result<Ty, String> {
    match c.peek() {
        Some('[') => {
            c.bump();
            let items = node(c, depth + 1)?;
            c.expect(']')?;
            Ok(Ty::Array(Box::new(items)))
        }
        Some('{') => {
            c.bump();
            let fields = fields(c, '}', depth)?;
            Ok(Ty::Object(Fields {
                fields,
                closed: c.eat('!'),
            }))
        }
        Some('\'') => {
            let first = EnumValue::Quoted(c.quoted()?);
            enumeration(c, first)
        }
        _ => {
            let atom = c.take_while(text::is_ident_char);
            if atom.is_empty() {
                return Err("missing type".into());
            }
            // A bare type keyword is never an enum value (those are quoted), so `str|null` is
            // a nullable string and not a two-value enum.
            let keyword = text::KEYWORDS.contains(&atom.as_str());
            if text::is_int_like(&atom) || (c.peek() == Some('|') && !keyword) {
                return enumeration(c, EnumValue::Bare(atom));
            }
            match atom.as_str() {
                // `num(1|2.5)` is an enum; `num(0..1)` is a range and is left for the caller.
                "num" if c.peek() == Some('(') && !c.peek_until(')').contains("..") => {
                    wrapped(c, |s| serde_json::from_str::<Number>(s).ok()).map(Ty::NumEnum)
                }
                "bool" if c.peek() == Some('(') => {
                    wrapped(c, |s| s.parse::<bool>().ok()).map(Ty::BoolEnum)
                }
                "str" if c.eat('<') => {
                    let format = c.take_while(text::is_ident_char);
                    c.expect('>')?;
                    if !schema::is_format(&format) {
                        return Err("empty format".into());
                    }
                    Ok(Ty::Str {
                        format: Some(format),
                    })
                }
                "str" => Ok(Ty::Str { format: None }),
                "datetime" => Ok(Ty::Str {
                    format: Some("date-time".into()),
                }),
                "int" => Ok(Ty::Int),
                "num" => Ok(Ty::Num),
                "bool" => Ok(Ty::Bool),
                "obj" => Ok(Ty::AnyObject),
                other => Err(format!("unknown type `{other}`")),
            }
        }
    }
}

enum EnumValue {
    Bare(String),
    Quoted(String),
}

fn enumeration(c: &mut Cursor, first: EnumValue) -> std::result::Result<Ty, String> {
    let mut values = vec![first];
    // A trailing `|null` marks the node nullable and is left for the caller; an enum value
    // spelled "null" is always quoted.
    while !c.at_null() && c.eat('|') {
        if c.peek() == Some('\'') {
            values.push(EnumValue::Quoted(c.quoted()?));
        } else {
            let atom = c.take_while(text::is_ident_char);
            if atom.is_empty() {
                return Err("missing enum value after `|`".into());
            }
            values.push(EnumValue::Bare(atom));
        }
    }
    // Bare integers are an integer enum; a string that merely looks like one is always quoted.
    let is_int = |v: &EnumValue| matches!(v, EnumValue::Bare(s) if text::is_int_like(s));
    if values.iter().all(is_int) {
        return values
            .iter()
            .map(|v| match v {
                EnumValue::Bare(s) => s
                    .parse::<i64>()
                    .map_err(|_| format!("`{s}` is out of range")),
                EnumValue::Quoted(_) => Err("quoted value in an integer enum".into()),
            })
            .collect::<std::result::Result<_, _>>()
            .map(Ty::IntEnum);
    }
    values
        .into_iter()
        .map(|v| match v {
            EnumValue::Quoted(s) => Ok(s),
            EnumValue::Bare(s) if text::is_ident(&s) => Ok(s),
            EnumValue::Bare(s) => Err(format!("enum mixes integer `{s}` with strings")),
        })
        .collect::<std::result::Result<_, _>>()
        .map(Ty::StrEnum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::encode_tools;
    use serde_json::{Value, json};

    fn compact(definitions: &str) -> CompactTools {
        CompactTools {
            definitions: definitions.into(),
            instructions: String::new(),
        }
    }

    fn round_trip(tool: &ToolDef) -> ToolDef {
        let encoded = encode_tools(std::slice::from_ref(tool)).unwrap();
        decode_tools(&encoded).unwrap().remove(0)
    }

    #[test]
    fn the_brief_example_line_parses_to_its_schema() {
        let line = "create_calendar_event(title:str, start:datetime, duration_min?:int, \
                    attendees?:[str], visibility?:public|private) - Create an event in the user's calendar.";
        let tools = decode_tools(&compact(line)).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "create_calendar_event");
        assert_eq!(
            tools[0].description.as_deref(),
            Some("Create an event in the user's calendar.")
        );
        let schema = tools[0].parameters.as_ref().unwrap();
        assert_eq!(
            schema["properties"]["start"],
            json!({"type": "string", "format": "date-time"})
        );
        assert_eq!(
            schema["properties"]["attendees"],
            json!({"type": "array", "items": {"type": "string"}})
        );
        assert_eq!(
            schema["properties"]["visibility"],
            json!({"type": "string", "enum": ["public", "private"]})
        );
        let mut required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        required.sort_unstable();
        assert_eq!(required, ["start", "title"]);
    }

    #[test]
    fn every_supported_feature_round_trips() {
        let tool = ToolDef {
            name: "ns.do-it_2".into(),
            description: Some("It's \"complicated\" - really (see docs).".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "plain": {"type": "string", "description": "A 'quoted' word, a comma, a ) and a }"},
                    "when": {"type": "string", "format": "date-time", "description": "ISO 8601"},
                    "mail": {"type": "string", "format": "email"},
                    "n": {"type": "integer"},
                    "x": {"type": "number", "description": "line\nbreak"},
                    "ok": {"type": "boolean"},
                    "level": {"type": "integer", "enum": [-1, 0, 7]},
                    "one": {"type": "string", "enum": ["only"]},
                    "mode": {"type": "string", "enum": ["a", "str", "two words", "42", "", "it's"]},
                    "free": {"type": "object", "description": "anything"},
                    "odd key?": {"type": "string"},
                    "list": {
                        "type": "array",
                        "description": "outer",
                        "items": {"type": "array", "items": {"type": "integer", "description": "inner"}}
                    },
                    "nested": {
                        "type": "object",
                        "description": "an object",
                        "properties": {
                            "id": {"type": "integer"},
                            "tags": {"type": "array", "items": {"type": "string", "enum": ["x", "y"]}},
                            "deep": {"type": "object", "properties": {}, "additionalProperties": false}
                        },
                        "required": ["id"],
                        "additionalProperties": false
                    }
                },
                "required": ["plain", "nested"],
                "additionalProperties": false
            })),
        };
        let back = round_trip(&tool);
        assert_eq!(back.name, tool.name);
        assert_eq!(back.description, tool.description);
        let (a, b) = (back.parameters.unwrap(), tool.parameters.unwrap());
        assert_eq!(a["properties"], b["properties"]);
        assert_eq!(a["additionalProperties"], json!(false));
    }

    #[test]
    fn awkward_descriptions_round_trip() {
        for description in [
            "",
            " leading",
            "trailing ",
            "'starts with a quote",
            "two\nlines",
            "a - b - c",
            "tab\there",
        ] {
            let tool = ToolDef {
                name: "t".into(),
                description: Some(description.into()),
                parameters: None,
            };
            assert_eq!(round_trip(&tool), tool, "lost {description:?}");
        }
    }

    #[test]
    fn no_parameters_and_empty_parameters_stay_distinct() {
        let bare = ToolDef {
            name: "ping".into(),
            description: None,
            parameters: None,
        };
        let empty = ToolDef {
            parameters: Some(json!({"type": "object", "properties": {}})),
            ..bare.clone()
        };
        assert_eq!(round_trip(&bare), bare);
        assert_eq!(round_trip(&empty), empty);
    }

    #[test]
    fn several_tools_keep_their_order() {
        let tools = decode_tools(&compact("b(x:int)\na\n\nc() - last")).unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["b", "a", "c"]);
    }

    #[test]
    fn lines_that_break_the_grammar_are_errors() {
        for bad in [
            "(x:int)",
            "t(x)",
            "t(x:)",
            "t(x:what)",
            "t(x:int",
            "t(x:int,)",
            "t(x:int, x:str)",
            "t(x:[int)",
            "t(x:{a:int)",
            "t(x:str<>)",
            "t(x:a|)",
            "t(x:1|b)",
            "t(x:99999999999999999999)",
            "t(x:int 'open)",
            "t(1x:int)",
            "t() -",
            "t() - ",
            "t() - 'quoted' extra",
            "t()x",
            "t\nt",
        ] {
            let result = decode_tools(&compact(bad));
            assert!(
                matches!(result, Err(CompactError::Malformed(_))),
                "accepted {bad:?}: {result:?}"
            );
        }
    }

    #[test]
    fn runaway_nesting_is_an_error_not_a_stack_overflow() {
        let line = format!("t(x:{}int{})", "[".repeat(5000), "]".repeat(5000));
        assert!(decode_tools(&compact(&line)).is_err());
    }
}
