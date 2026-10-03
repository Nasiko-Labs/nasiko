//! Shared test support: a seeded generator of supported schemas, valid instances for them, and
//! mutations that should make an instance invalid.
//!
//! Everything is deterministic (a hand-rolled LCG) so failures reproduce from a seed and the
//! crate stays free of RNG dependencies.

#![allow(dead_code)]

use nasiko_tool_compact::ToolDef;
use serde_json::{Map, Value, json};

/// 64-bit linear congruential generator (Knuth's MMIX constants).
pub struct Lcg(pub u64);

impl Lcg {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

const WORDS: &[&str] = &[
    "alpha",
    "beta",
    "gamma",
    "delta",
    "x",
    "y_z",
    "with.dot",
    "k-v",
    "λx",
    "日本語",
    "emoji 🎉",
    "has \"quotes\"",
    "back\\slash",
    "two\nlines",
    " leading",
    "trailing ",
    "",
    "str",
    "int",
    "null",
    "2",
    "-1",
    "zh-Hans",
    "public",
    "private",
];

const DESCRIPTIONS: &[&str] = &[
    "Plain description.",
    "Mit Umlauten: äöü ß",
    "日本語の説明",
    "Has \"quotes\" and a \\ backslash",
    "Line one\nLine two",
    " leading space",
    "trailing space ",
    "",
    "Emoji 🎉 and tab\tinside",
    "Ends with a quote\"",
    "\"Starts with a quote",
];

const FORMATS: &[&str] = &[
    "date-time",
    "date",
    "email",
    "uri",
    "uuid",
    "ipv4",
    "my-format",
    "x y",
];

fn desc(rng: &mut Lcg) -> Option<&'static str> {
    if rng.chance(50) {
        Some(rng.pick(DESCRIPTIONS))
    } else {
        None
    }
}

fn with_meta(rng: &mut Lcg, mut schema: Map<String, Value>) -> Value {
    if let Some(d) = desc(rng) {
        schema.insert("description".into(), json!(d));
    }
    if rng.chance(15) {
        schema.insert("title".into(), json!(rng.pick(WORDS)));
    }
    if rng.chance(15) {
        let d = match schema.get("type").and_then(Value::as_str) {
            Some("string") => json!("dflt"),
            Some("integer") => json!(3),
            Some("number") => json!(2.5),
            Some("boolean") => json!(true),
            Some("array") => json!([]),
            _ => json!({"k": [1, "two", null]}),
        };
        schema.insert("default".into(), d);
    }
    if rng.chance(10) {
        schema.insert("examples".into(), json!(["a", 1, {"nested": true}]));
    }
    Value::Object(schema)
}

fn type_value(rng: &mut Lcg, t: &str) -> (Value, bool) {
    if rng.chance(25) {
        (json!([t, "null"]), true)
    } else {
        (json!(t), false)
    }
}

/// A random schema from the supported dialect.
pub fn gen_schema(rng: &mut Lcg, depth: usize) -> Value {
    let choice = rng.below(if depth >= 3 { 7 } else { 10 });
    let mut m = Map::new();
    match choice {
        0 | 1 => {
            let (t, _) = type_value(rng, "string");
            m.insert("type".into(), t);
            if rng.chance(30) {
                m.insert("format".into(), json!(rng.pick(FORMATS)));
            }
            if rng.chance(20) {
                m.insert("minLength".into(), json!(rng.below(3)));
            }
            if rng.chance(20) {
                m.insert("maxLength".into(), json!(5 + rng.below(100)));
            }
        }
        2 => {
            let (t, _) = type_value(rng, "integer");
            m.insert("type".into(), t);
            numeric_bounds(rng, &mut m, true);
        }
        3 => {
            let (t, _) = type_value(rng, "number");
            m.insert("type".into(), t);
            numeric_bounds(rng, &mut m, false);
        }
        4 => {
            let (t, _) = type_value(rng, "boolean");
            m.insert("type".into(), t);
        }
        5 => {
            // string enum
            let (t, nullable) = type_value(rng, "string");
            m.insert("type".into(), t);
            let n = 1 + rng.below(4) as usize;
            let mut members: Vec<Value> = Vec::new();
            for _ in 0..n {
                let w = *rng.pick(WORDS);
                if !members.iter().any(|m| m == &json!(w)) {
                    members.push(json!(w));
                }
            }
            if nullable {
                let at = rng.below(members.len() as u64 + 1) as usize;
                members.insert(at, Value::Null);
            }
            m.insert("enum".into(), Value::Array(members));
        }
        6 => {
            // integer enum
            let (t, nullable) = type_value(rng, "integer");
            m.insert("type".into(), t);
            let mut members: Vec<Value> = vec![json!(1), json!(-7), json!(9007199254740993_i64)];
            members.truncate(1 + rng.below(3) as usize);
            if nullable {
                members.push(Value::Null);
            }
            m.insert("enum".into(), Value::Array(members));
        }
        7 | 8 => {
            let (t, _) = type_value(rng, "array");
            m.insert("type".into(), t);
            if rng.chance(85) {
                m.insert("items".into(), gen_schema(rng, depth + 1));
            }
            if rng.chance(25) {
                m.insert("minItems".into(), json!(rng.below(2)));
            }
            if rng.chance(25) {
                m.insert("maxItems".into(), json!(3 + rng.below(5)));
            }
        }
        _ => {
            let nullable = rng.chance(25);
            return gen_object(rng, depth + 1, nullable);
        }
    }
    with_meta(rng, m)
}

fn numeric_bounds(rng: &mut Lcg, m: &mut Map<String, Value>, integer: bool) {
    let lo: i64 = -5 + rng.below(10) as i64;
    if rng.chance(40) {
        m.insert("minimum".into(), json!(lo));
    } else if rng.chance(30) {
        m.insert(
            "exclusiveMinimum".into(),
            if integer {
                json!(lo)
            } else {
                json!(lo as f64 + 0.5)
            },
        );
    }
    if rng.chance(40) {
        m.insert("maximum".into(), json!(lo + 10 + rng.below(100) as i64));
    } else if rng.chance(30) {
        m.insert("exclusiveMaximum".into(), json!(lo + 20));
    }
}

/// A random object schema (used both at the root and nested).
pub fn gen_object(rng: &mut Lcg, depth: usize, nullable: bool) -> Value {
    let mut m = Map::new();
    m.insert(
        "type".into(),
        if nullable {
            json!(["object", "null"])
        } else {
            json!("object")
        },
    );
    let variant = rng.below(10);
    if variant == 0 {
        // {"type":"object"} with no properties key
    } else {
        let n = if variant == 1 {
            0
        } else {
            1 + rng.below(5) as usize
        };
        let mut props = Map::new();
        let mut names: Vec<String> = Vec::new();
        for i in 0..n {
            let name = match rng.below(6) {
                0 => "snake_case".to_owned(),
                1 => "with.dot".to_owned(),
                2 => "x-y".to_owned(),
                3 => "_under".to_owned(),
                _ => format!("p{i}"),
            };
            if props.contains_key(&name) {
                continue;
            }
            props.insert(name.clone(), gen_schema(rng, depth + 1));
            names.push(name);
        }
        m.insert("properties".into(), Value::Object(props));
        if rng.chance(60) {
            // Random subset in shuffled order.
            let mut req: Vec<String> = Vec::new();
            for name in &names {
                if rng.chance(60) {
                    req.push(name.clone());
                }
            }
            for i in (1..req.len()).rev() {
                let j = rng.below(i as u64 + 1) as usize;
                req.swap(i, j);
            }
            m.insert(
                "required".into(),
                Value::Array(req.into_iter().map(Value::String).collect()),
            );
        } else if rng.chance(20) {
            m.insert("required".into(), json!([]));
        }
    }
    if rng.chance(35) {
        m.insert("additionalProperties".into(), json!(rng.chance(60)));
    }
    with_meta(rng, m)
}

/// A random tool whose parameters are a supported root schema (or absent, or `{}`).
pub fn gen_tool(rng: &mut Lcg, index: usize) -> ToolDef {
    let name = match rng.below(5) {
        0 => format!("tool_{index}"),
        1 => format!("v2.tool-{index}"),
        2 => format!("{index}leading_digit"),
        _ => format!("do_thing_{index}"),
    };
    let parameters = match rng.below(12) {
        0 => None,
        1 => Some(json!({})),
        2 => Some(json!({"description": "anything"})),
        _ => Some(gen_object(rng, 0, false)),
    };
    ToolDef {
        name,
        description: desc(rng).map(str::to_owned),
        parameters,
    }
}

/// Build a valid instance for a supported schema value.
pub fn gen_instance(rng: &mut Lcg, schema: &Value) -> Value {
    let obj = schema.as_object().expect("schema object");
    let nullable = matches!(obj.get("type"), Some(Value::Array(_)));
    let t = match obj.get("type") {
        None => return json!({"anything": [1, "goes", null]}),
        Some(Value::String(t)) => t.as_str(),
        Some(Value::Array(a)) => a[0].as_str().unwrap_or(""),
        _ => "",
    };
    if let Some(Value::Array(members)) = obj.get("enum") {
        return rng.pick(members).clone();
    }
    if nullable && rng.chance(20) {
        return Value::Null;
    }
    match t {
        "string" => {
            let min = obj.get("minLength").and_then(Value::as_u64).unwrap_or(0) as usize;
            let max = obj.get("maxLength").and_then(Value::as_u64).unwrap_or(8) as usize;
            let len = min.max(max.min(min + 3));
            Value::String("é".repeat(len))
        }
        "integer" | "number" => {
            let lo = obj
                .get("minimum")
                .and_then(Value::as_f64)
                .or_else(|| {
                    obj.get("exclusiveMinimum")
                        .and_then(Value::as_f64)
                        .map(|v| v + 1.0)
                })
                .unwrap_or(0.0);
            let hi = obj
                .get("maximum")
                .and_then(Value::as_f64)
                .or_else(|| {
                    obj.get("exclusiveMaximum")
                        .and_then(Value::as_f64)
                        .map(|v| v - 1.0)
                })
                .unwrap_or(lo + 10.0);
            let v = lo.ceil().max(hi.floor().min(lo.ceil() + 1.0));
            if t == "integer" || rng.chance(50) {
                json!(v as i64)
            } else {
                json!(v + 0.25)
            }
        }
        "boolean" => json!(rng.chance(50)),
        "null" => Value::Null,
        "array" => {
            let min = obj.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
            let max = obj.get("maxItems").and_then(Value::as_u64).unwrap_or(3) as usize;
            let len = min.max(max.min(min + 1));
            let items: Vec<Value> = (0..len)
                .map(|_| match obj.get("items") {
                    Some(items) => gen_instance(rng, items),
                    None => json!("free"),
                })
                .collect();
            Value::Array(items)
        }
        "object" => {
            let mut out = Map::new();
            let props = obj.get("properties").and_then(Value::as_object);
            let required: Vec<&str> = obj
                .get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if let Some(props) = props {
                for (name, schema) in props {
                    if required.contains(&name.as_str()) || rng.chance(50) {
                        out.insert(name.clone(), gen_instance(rng, schema));
                    }
                }
            }
            if obj.get("additionalProperties") != Some(&json!(false)) && rng.chance(20) {
                out.insert("extra_key".into(), json!("allowed"));
            }
            Value::Object(out)
        }
        _ => Value::Null,
    }
}

/// Walk a root object schema and its instance, returning candidate `(pointer, schema)` pairs for
/// properties that are present in the instance.
fn present_props<'a>(
    schema: &'a Value,
    instance: &'a Value,
    path: &str,
    out: &mut Vec<(String, &'a Value, &'a Value)>,
) {
    let (Some(props), Some(inst)) = (
        schema.get("properties").and_then(Value::as_object),
        instance.as_object(),
    ) else {
        return;
    };
    for (name, child) in props {
        if let Some(v) = inst.get(name) {
            let p = format!("{path}/{name}");
            out.push((p.clone(), child, v));
            if child.get("type") == Some(&json!("object")) {
                present_props(child, v, &p, out);
            }
        }
    }
}

fn set_at(instance: &mut Value, pointer: &str, value: Option<Value>) {
    let parts: Vec<&str> = pointer.split('/').filter(|p| !p.is_empty()).collect();
    let mut cur = instance;
    for part in &parts[..parts.len() - 1] {
        cur = cur.get_mut(*part).expect("path exists");
    }
    let last = parts[parts.len() - 1];
    let map = cur.as_object_mut().expect("object at path");
    match value {
        Some(v) => {
            map.insert(last.to_owned(), v);
        }
        None => {
            map.remove(last);
        }
    }
}

fn base_type(schema: &Value) -> Option<&str> {
    match schema.get("type") {
        Some(Value::String(t)) => Some(t.as_str()),
        Some(Value::Array(a)) => a[0].as_str(),
        _ => None,
    }
}

/// Remove a required property (root or nested). `None` when nothing is required.
pub fn drop_required(schema: &Value, instance: &Value) -> Option<Value> {
    let mut candidates = Vec::new();
    collect_required(schema, instance, "", &mut candidates);
    let pointer = candidates.first()?.clone();
    let mut out = instance.clone();
    set_at(&mut out, &pointer, None);
    Some(out)
}

fn collect_required(schema: &Value, instance: &Value, path: &str, out: &mut Vec<String>) {
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for name in required {
        if instance.get(name).is_some() {
            out.push(format!("{path}/{name}"));
        }
    }
    let mut present = Vec::new();
    present_props(schema, instance, path, &mut present);
    for (p, child, v) in present {
        if base_type(child) == Some("object") && v.is_object() {
            collect_required(child, v, &p, out);
        }
    }
}

/// Replace one present property's value with a value of a definitely wrong type.
pub fn wrong_type(schema: &Value, instance: &Value) -> Option<Value> {
    let mut present = Vec::new();
    present_props(schema, instance, "", &mut present);
    for (p, child, v) in present {
        if child.get("type").is_none() || child.get("enum").is_some() || v.is_null() {
            continue;
        }
        let wrong = match base_type(child)? {
            "string" => json!(12345),
            "integer" | "number" => json!("12345"),
            "boolean" => json!("true"),
            "array" => json!({"not": "an array"}),
            "object" => json!(["not", "an", "object"]),
            "null" => json!(0),
            _ => continue,
        };
        let mut out = instance.clone();
        set_at(&mut out, &p, Some(wrong));
        return Some(out);
    }
    None
}

/// Replace an enum property's value with a non-member.
pub fn bad_enum(schema: &Value, instance: &Value) -> Option<Value> {
    let mut present = Vec::new();
    present_props(schema, instance, "", &mut present);
    for (p, child, _) in present {
        if let Some(members) = child.get("enum").and_then(Value::as_array) {
            let bad = if members.iter().any(Value::is_string) {
                json!("__not_a_member__")
            } else {
                json!(123456789)
            };
            let mut out = instance.clone();
            set_at(&mut out, &p, Some(bad));
            return Some(out);
        }
    }
    None
}

/// Add a property to an object that forbids additional properties.
pub fn forbidden_extra(schema: &Value, instance: &Value) -> Option<Value> {
    if schema.get("additionalProperties") == Some(&json!(false)) {
        let mut out = instance.clone();
        out.as_object_mut()?.insert("__extra__".into(), json!(1));
        return Some(out);
    }
    let mut present = Vec::new();
    present_props(schema, instance, "", &mut present);
    for (p, child, v) in present {
        if child.get("additionalProperties") == Some(&json!(false)) && v.is_object() {
            let mut out = instance.clone();
            let mut inner = v.clone();
            inner.as_object_mut()?.insert("__extra__".into(), json!(1));
            set_at(&mut out, &p, Some(inner));
            return Some(out);
        }
    }
    None
}

/// Drop a required property inside a nested object only.
pub fn break_nested(schema: &Value, instance: &Value) -> Option<Value> {
    let mut present = Vec::new();
    present_props(schema, instance, "", &mut present);
    for (p, child, v) in present {
        if base_type(child) == Some("object") && v.is_object() {
            let mut nested = Vec::new();
            collect_required(child, v, &p, &mut nested);
            if let Some(pointer) = nested.first() {
                let mut out = instance.clone();
                set_at(&mut out, pointer, None);
                return Some(out);
            }
        }
    }
    None
}

/// Violate a numeric or length bound.
pub fn violate_bound(schema: &Value, instance: &Value) -> Option<Value> {
    let mut present = Vec::new();
    present_props(schema, instance, "", &mut present);
    for (p, child, v) in present {
        if v.is_null() || child.get("enum").is_some() {
            continue;
        }
        let bad = match base_type(child)? {
            "integer" | "number" => {
                if let Some(min) = child.get("minimum").and_then(Value::as_f64) {
                    json!(min - 1.0)
                } else if let Some(min) = child.get("exclusiveMinimum").and_then(Value::as_f64) {
                    json!(min)
                } else if let Some(max) = child.get("maximum").and_then(Value::as_f64) {
                    json!(max + 1.0)
                } else if let Some(max) = child.get("exclusiveMaximum").and_then(Value::as_f64) {
                    json!(max)
                } else {
                    continue;
                }
            }
            "string" => {
                if let Some(max) = child.get("maxLength").and_then(Value::as_u64) {
                    json!("x".repeat(max as usize + 1))
                } else if let Some(min) = child.get("minLength").and_then(Value::as_u64) {
                    if min == 0 {
                        continue;
                    }
                    json!("x".repeat(min as usize - 1))
                } else {
                    continue;
                }
            }
            "array" => {
                if let Some(max) = child.get("maxItems").and_then(Value::as_u64) {
                    Value::Array(vec![json!(null); max as usize + 1])
                } else if let Some(min) = child.get("minItems").and_then(Value::as_u64) {
                    if min == 0 {
                        continue;
                    }
                    Value::Array(vec![])
                } else {
                    continue;
                }
            }
            _ => continue,
        };
        let mut out = instance.clone();
        set_at(&mut out, &p, Some(bad));
        return Some(out);
    }
    None
}

pub type Mutator = fn(&Value, &Value) -> Option<Value>;

pub const MUTATORS: &[(&str, Mutator)] = &[
    ("drop_required", drop_required),
    ("wrong_type", wrong_type),
    ("bad_enum", bad_enum),
    ("forbidden_extra", forbidden_extra),
    ("break_nested", break_nested),
    ("violate_bound", violate_bound),
];

/// Convenience: a single-tool catalog around a root schema.
pub fn tool(name: &str, parameters: Value) -> ToolDef {
    ToolDef {
        name: name.to_owned(),
        description: Some("test tool".to_owned()),
        parameters: Some(parameters),
    }
}
