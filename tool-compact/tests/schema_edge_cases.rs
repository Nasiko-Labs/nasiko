//! Schema preservation on awkward-but-legal inputs. Every tool the encoder accepts must decode
//! back (via `decode_tools`) to an equivalent schema; anything it cannot preserve must be
//! bypassed instead. "Equivalent" ignores only representation differences that cannot change
//! validation: surrounding whitespace in descriptions, an empty description, an explicit empty
//! `required`/`properties`, and a missing top-level schema versus `{"type":"object"}`.

use nasiko_tool_compact::{ToolDef, decode_tools, encode_tools};
use serde_json::{Map, Value, json};

fn norm_schema(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, child) in m {
                match k.as_str() {
                    "description" => {
                        if let Some(t) = child.as_str().map(str::trim).filter(|t| !t.is_empty()) {
                            out.insert(k.clone(), json!(t));
                        }
                    }
                    "required" if child.as_array().is_some_and(Vec::is_empty) => {}
                    "properties" => {
                        let props: Map<String, Value> = child
                            .as_object()
                            .map(|p| p.iter().map(|(n, s)| (n.clone(), norm_schema(s))).collect())
                            .unwrap_or_default();
                        out.insert(k.clone(), Value::Object(props));
                    }
                    _ => {
                        out.insert(k.clone(), norm_schema(child));
                    }
                }
            }
            // An enum with no type is a string enum here.
            if out.contains_key("enum") && !out.contains_key("type") {
                out.insert("type".into(), json!("string"));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(norm_schema).collect()),
        other => other.clone(),
    }
}

fn norm_tool(t: &ToolDef) -> (String, Option<String>, Value) {
    let desc = t
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(String::from);
    let mut params = t
        .parameters
        .clone()
        .unwrap_or_else(|| json!({"type": "object"}));
    if params
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(Map::is_empty)
    {
        params.as_object_mut().map(|m| m.remove("properties"));
    }
    (t.name.clone(), desc, norm_schema(&params))
}

/// Ok(true) = encoded and preserved; Ok(false) = bypassed (allowed); Err = defect.
fn check(tool: &ToolDef) -> Result<bool, String> {
    let encoded =
        encode_tools(std::slice::from_ref(tool)).map_err(|e| format!("encode error: {e}"))?;
    if !encoded.bypassed.is_empty() {
        return Ok(false);
    }
    let back = decode_tools(&encoded)
        .map_err(|e| format!("decode_tools failed: {e}\n{}", encoded.text))?;
    if back.len() != 1 {
        return Err(format!("expected 1 tool back, got {}", back.len()));
    }
    let (a, b) = (norm_tool(tool), norm_tool(&back[0]));
    if a == b {
        Ok(true)
    } else {
        Err(format!(
            "CHANGED\n  before: {a:?}\n  after:  {b:?}\n  text:\n{}",
            encoded.text
        ))
    }
}

fn tool(desc: Option<&str>, params: Value) -> ToolDef {
    ToolDef {
        name: "t".into(),
        description: desc.map(String::from),
        parameters: Some(params),
    }
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

fn run(name: &str, t: ToolDef, failures: &mut Vec<String>) {
    if let Err(e) = check(&t) {
        failures.push(format!("[{name}] {e}"));
    }
}

#[test]
fn awkward_but_legal_schemas_are_preserved_or_bypassed() {
    let mut f: Vec<String> = Vec::new();
    // enum value shapes
    for v in [
        "a:b",
        "x+y",
        "a/b",
        "1",
        "42",
        "true",
        "null",
        "Asia/Kolkata",
        "A_B-c.d",
        "str",
        "int",
        "datetime",
        "x:y:z",
    ] {
        run(
            &format!("enum single {v}"),
            tool(
                None,
                obj(json!({"p": {"type": "string", "enum": [v]}}), &["p"]),
            ),
            &mut f,
        );
        run(
            &format!("enum pair {v}"),
            tool(
                None,
                obj(json!({"p": {"type": "string", "enum": [v, "zz"]}}), &[]),
            ),
            &mut f,
        );
    }
    // descriptions
    let descs = [
        "a: b",
        "x - y",
        "f(x)",
        "list[]",
        "has \"quotes\" and 'single'",
        "back\\slash",
        "é 😀",
        "  padded  ",
        "",
        "   ",
        "ends with colon:",
        "- starts with dash",
        "(parens) - and: both",
        "a.b.c",
    ];
    for d in descs {
        run(
            &format!("prop desc {d:?}"),
            tool(
                None,
                obj(json!({"p": {"type": "string", "description": d}}), &["p"]),
            ),
            &mut f,
        );
        run(
            &format!("tool desc {d:?}"),
            tool(Some(d), obj(json!({"p": {"type": "string"}}), &["p"])),
            &mut f,
        );
        run(
            &format!("nested desc {d:?}"),
            tool(
                None,
                obj(
                    json!({"o": {"type": "object", "properties": {"q": {"type": "integer", "description": d}}, "required": ["q"]}}),
                    &["o"],
                ),
            ),
            &mut f,
        );
        run(
            &format!("array-item desc {d:?}"),
            tool(
                None,
                obj(
                    json!({"a": {"type": "array", "items": {"type": "object", "properties": {"q": {"type": "number", "description": d}}}}}),
                    &[],
                ),
            ),
            &mut f,
        );
    }
    // names
    for n in [
        "a-b",
        "A1",
        "_x",
        "description",
        "type",
        "properties",
        "required",
        "items",
        "x_y_z_long_name_123",
    ] {
        run(
            &format!("prop name {n}"),
            tool(
                None,
                obj(json!({n: {"type": "string", "description": "d"}}), &[n]),
            ),
            &mut f,
        );
    }
    // structure
    run(
        "no params",
        ToolDef {
            name: "t".into(),
            description: Some("d".into()),
            parameters: None,
        },
        &mut f,
    );
    run(
        "empty object",
        tool(None, json!({"type": "object"})),
        &mut f,
    );
    run(
        "empty properties",
        tool(None, json!({"type": "object", "properties": {}})),
        &mut f,
    );
    run(
        "empty required",
        tool(
            None,
            json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": []}),
        ),
        &mut f,
    );
    run(
        "enum no type",
        tool(None, obj(json!({"p": {"enum": ["a", "b"]}}), &[])),
        &mut f,
    );
    run(
        "array of arrays of objects",
        tool(
            None,
            obj(
                json!({"m": {"type": "array", "items": {"type": "array", "items": {"type": "object", "properties": {"z": {"type": "string", "description": "deep"}}, "required": ["z"]}}}}),
                &["m"],
            ),
        ),
        &mut f,
    );
    run(
        "object in object in array",
        tool(
            None,
            obj(
                json!({"a": {"type": "array", "items": {"type": "object", "properties": {"b": {"type": "object", "properties": {"c": {"type": "boolean", "description": "flag"}}, "required": ["c"]}}, "required": ["b"]}}}),
                &["a"],
            ),
        ),
        &mut f,
    );
    run(
        "datetime array",
        tool(
            None,
            obj(
                json!({"d": {"type": "array", "items": {"type": "string", "format": "date-time"}}}),
                &["d"],
            ),
        ),
        &mut f,
    );
    run(
        "empty nested object",
        tool(
            None,
            obj(json!({"o": {"type": "object", "properties": {}}}), &["o"]),
        ),
        &mut f,
    );
    run(
        "all optional",
        tool(
            None,
            obj(
                json!({"b": {"type": "string"}, "a": {"type": "integer"}}),
                &[],
            ),
        ),
        &mut f,
    );
    run(
        "required out of alpha order",
        tool(
            None,
            obj(
                json!({"a": {"type": "string"}, "z": {"type": "string"}, "m": {"type": "string"}}),
                &["z", "a"],
            ),
        ),
        &mut f,
    );
    assert!(
        f.is_empty(),
        "{} schema-preservation defects:\n{}",
        f.len(),
        f.join("\n\n")
    );
}
