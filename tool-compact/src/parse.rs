//! One canonical line of compact notation → [`ToolDef`].
//!
//! This parser reads the crate's own output (never model or user text), so it is strict: it
//! accepts exactly the canonical form [`crate::render`] emits and reports anything else as an
//! error with a byte position. Strictness is what lets `encode_tools` prove, by re-parsing, that
//! a rendered line carries its schema faithfully.

use serde_json::Value;

use crate::lexeme::{Cursor, ParseError, format_for_alias, is_arg_name, is_tool_name};
use crate::schema::{Annot, EnumBase, Kind, Node, ObjectSchema, Scalar, ScalarBase};
use crate::types::ToolDef;

/// Parse one line into a tool definition.
pub(crate) fn parse_tool(line: &str) -> Result<ToolDef, ParseError> {
    let mut c = Cursor::new(line);
    let name = c.take_name();
    if !is_tool_name(name) {
        return Err(c.err("expected a tool name"));
    }
    let name = name.to_owned();

    let parameters = if c.starts_with("(") {
        Some(parse_root(&mut c)?.to_value())
    } else {
        None
    };

    let description = if c.at_end() {
        None
    } else {
        c.expect(" - ")?;
        if c.starts_with("\"") {
            Some(c.take_json_string()?)
        } else {
            // A raw description runs to the end of the line.
            Some(c.take_while(|_| true).to_owned())
        }
    };
    if !c.at_end() {
        return Err(c.err("unexpected trailing text"));
    }

    Ok(ToolDef {
        name,
        description,
        parameters,
    })
}

fn parse_root(c: &mut Cursor<'_>) -> Result<Node, ParseError> {
    c.expect("(")?;
    let kind = if c.eat("*)") {
        Kind::Any
    } else {
        let body = parse_object_body(c, ')')?;
        Kind::Object(parse_flags(c, body))
    };
    let annot = parse_annot(c)?;
    let description = if c.starts_with(" \"") || c.starts_with(" '") {
        c.expect(" ")?;
        Some(c.take_description()?)
    } else {
        None
    };
    Ok(Node {
        kind,
        nullable: false,
        description,
        annot,
    })
}

/// Parses `...CLOSE`, `CLOSE` or `arg, arg, ...CLOSE` and consumes the closing delimiter.
fn parse_object_body(c: &mut Cursor<'_>, close: char) -> Result<ObjectSchema, ParseError> {
    let close_str = close.to_string();
    if c.eat("...") {
        c.expect(&close_str)?;
        return Ok(ObjectSchema {
            properties: None,
            required: None,
            additional: None,
        });
    }
    let mut properties: Vec<(String, Node)> = Vec::new();
    let mut required: Vec<String> = Vec::new();
    let mut seen_optional = false;
    if !c.eat(&close_str) {
        loop {
            let name = c.take_name().to_owned();
            if !is_arg_name(&name) {
                return Err(c.err("expected a property name"));
            }
            if properties.iter().any(|(n, _)| *n == name) {
                return Err(c.err(format!("property `{name}` appears twice")));
            }
            let optional = c.eat("?");
            if optional {
                seen_optional = true;
            } else if seen_optional {
                return Err(c.err("required properties must come before optional ones"));
            }
            c.expect(":")?;
            let node = parse_typed(c)?;
            if !optional {
                required.push(name.clone());
            }
            properties.push((name, node));
            if c.eat(&close_str) {
                break;
            }
            c.expect(", ")?;
        }
    }
    // Optional properties must be in sorted order; required ones follow the `required` array.
    let optional_names: Vec<&str> = properties
        .iter()
        .filter(|(n, _)| !required.contains(n))
        .map(|(n, _)| n.as_str())
        .collect();
    if optional_names.windows(2).any(|w| w[0] >= w[1]) {
        return Err(c.err("optional properties must be sorted by name"));
    }
    properties.sort_by(|(a, _), (b, _)| a.cmp(b));
    Ok(ObjectSchema {
        properties: Some(properties),
        required: if required.is_empty() {
            None
        } else {
            Some(required)
        },
        additional: None,
    })
}

fn parse_flags(c: &mut Cursor<'_>, mut o: ObjectSchema) -> ObjectSchema {
    if c.eat("!") {
        o.additional = Some(false);
    } else if c.eat("+") {
        o.additional = Some(true);
    }
    if c.eat("=") && o.required.is_none() {
        o.required = Some(Vec::new());
    }
    o
}

fn parse_annot(c: &mut Cursor<'_>) -> Result<Annot, ParseError> {
    if !c.eat("@") {
        return Ok(Annot::default());
    }
    let start = c.pos();
    let value = c.take_json_value()?;
    let Value::Object(map) = value else {
        return Err(ParseError {
            pos: start,
            reason: "annotations must be a JSON object".into(),
        });
    };
    let mut annot = Annot::default();
    for (k, v) in map {
        match (k.as_str(), v) {
            ("title", Value::String(s)) => annot.title = Some(s),
            ("default", v) => annot.default = Some(v),
            ("examples", v) => annot.examples = Some(v),
            (k, _) => {
                return Err(ParseError {
                    pos: start,
                    reason: format!("unknown annotation `{k}`"),
                });
            }
        }
    }
    Ok(annot)
}

/// A type expression with its optional `@{...}` annotations and `"description"`.
fn parse_typed(c: &mut Cursor<'_>) -> Result<Node, ParseError> {
    let mut node = parse_type(c)?;
    node.annot = parse_annot(c)?;
    if c.starts_with(" \"") || c.starts_with(" '") {
        c.expect(" ")?;
        node.description = Some(c.take_description()?);
    }
    Ok(node)
}

fn parse_type(c: &mut Cursor<'_>) -> Result<Node, ParseError> {
    let mut node = if c.eat("*") {
        Node {
            kind: Kind::Any,
            nullable: false,
            description: None,
            annot: Annot::default(),
        }
    } else if c.starts_with("[") {
        parse_array(c)?
    } else if c.starts_with("{") {
        c.expect("{")?;
        let body = parse_object_body(c, '}')?;
        let o = parse_flags(c, body);
        Node {
            kind: Kind::Object(o),
            nullable: false,
            description: None,
            annot: Annot::default(),
        }
    } else if c.starts_with("\"") || c.peek().is_some_and(|ch| ch.is_ascii_digit() || ch == '-') {
        return parse_enum(c, None);
    } else {
        let word = c.take_name().to_owned();
        match word.as_str() {
            "str" => parse_str(c, None)?,
            "int" => parse_numeric(c, ScalarBase::Int)?,
            "num" => parse_numeric(c, ScalarBase::Num)?,
            "bool" => scalar(ScalarBase::Bool),
            // `null|…` is an enum whose first member is null; a lone `null` is the null type.
            "null" if c.starts_with("|") => return parse_enum(c, Some(Value::Null)),
            "null" => scalar(ScalarBase::Null),
            alias if format_for_alias(alias).is_some() => {
                parse_str(c, Some(format_for_alias(alias).unwrap_or("")))?
            }
            _ => {
                // A bare word that is not a type is the first member of an enum.
                return parse_enum(c, Some(Value::String(word)));
            }
        }
    };
    if node.kind != Kind::Any && c.eat("|null") {
        node.nullable = true;
    }
    Ok(node)
}

fn scalar(base: ScalarBase) -> Node {
    Node {
        kind: Kind::Scalar(Scalar {
            base,
            ..Scalar::default()
        }),
        nullable: false,
        description: None,
        annot: Annot::default(),
    }
}

fn parse_str(c: &mut Cursor<'_>, alias_format: Option<&str>) -> Result<Node, ParseError> {
    let mut s = Scalar {
        base: ScalarBase::Str,
        format: alias_format.map(str::to_owned),
        ..Scalar::default()
    };
    if c.eat("(") {
        let mut first = true;
        loop {
            if !first {
                c.expect(",")?;
            }
            first = false;
            if c.eat("min=") {
                s.min_length = Some(c.take_u64()?);
            } else if c.eat("max=") {
                s.max_length = Some(c.take_u64()?);
            } else if c.eat("format=") {
                if alias_format.is_some() {
                    return Err(c.err("format alias cannot also carry format="));
                }
                s.format = Some(if c.starts_with("\"") {
                    c.take_json_string()?
                } else {
                    c.take_name().to_owned()
                });
            } else {
                return Err(c.err("expected min=, max= or format="));
            }
            if c.eat(")") {
                break;
            }
        }
    }
    Ok(Node {
        kind: Kind::Scalar(s),
        nullable: false,
        description: None,
        annot: Annot::default(),
    })
}

fn parse_numeric(c: &mut Cursor<'_>, base: ScalarBase) -> Result<Node, ParseError> {
    let mut s = Scalar {
        base,
        ..Scalar::default()
    };
    if c.eat("(") {
        if c.starts_with("ge=")
            || c.starts_with("gt=")
            || c.starts_with("le=")
            || c.starts_with("lt=")
        {
            let mut first = true;
            loop {
                if !first {
                    c.expect(",")?;
                }
                first = false;
                if c.eat("ge=") {
                    s.minimum = Some(c.take_number()?);
                } else if c.eat("gt=") {
                    s.exclusive_minimum = Some(c.take_number()?);
                } else if c.eat("le=") {
                    s.maximum = Some(c.take_number()?);
                } else if c.eat("lt=") {
                    s.exclusive_maximum = Some(c.take_number()?);
                } else {
                    return Err(c.err("expected ge=, gt=, le= or lt="));
                }
                if c.eat(")") {
                    break;
                }
            }
            if !s.has_exclusive_bounds() {
                return Err(c.err("ge=/le= form requires an exclusive bound"));
            }
        } else {
            if !c.starts_with("..") {
                s.minimum = Some(c.take_number()?);
            }
            c.expect("..")?;
            if !c.starts_with(")") {
                s.maximum = Some(c.take_number()?);
            }
            c.expect(")")?;
            if s.minimum.is_none() && s.maximum.is_none() {
                return Err(c.err("empty range"));
            }
        }
    }
    Ok(Node {
        kind: Kind::Scalar(s),
        nullable: false,
        description: None,
        annot: Annot::default(),
    })
}

fn parse_array(c: &mut Cursor<'_>) -> Result<Node, ParseError> {
    c.expect("[")?;
    let items = if c.eat("...") {
        None
    } else {
        Some(Box::new(parse_typed(c)?))
    };
    c.expect("]")?;
    let (mut min_items, mut max_items) = (None, None);
    if c.eat("(") {
        if !c.starts_with("..") {
            min_items = Some(c.take_u64()?);
        }
        c.expect("..")?;
        if !c.starts_with(")") {
            max_items = Some(c.take_u64()?);
        }
        c.expect(")")?;
        if min_items.is_none() && max_items.is_none() {
            return Err(c.err("empty range"));
        }
    }
    Ok(Node {
        kind: Kind::Array {
            items,
            min_items,
            max_items,
        },
        nullable: false,
        description: None,
        annot: Annot::default(),
    })
}

fn parse_enum(c: &mut Cursor<'_>, first: Option<Value>) -> Result<Node, ParseError> {
    let mut members: Vec<Value> = Vec::new();
    let mut base: Option<EnumBase> = None;
    let mut pending = first;
    loop {
        let member = match pending.take() {
            Some(m) => m,
            None => {
                if c.eat("null") {
                    Value::Null
                } else if c.starts_with("\"") {
                    Value::String(c.take_json_string()?)
                } else if c.peek().is_some_and(|ch| ch.is_ascii_digit() || ch == '-') {
                    Value::Number(c.take_number()?)
                } else {
                    let word = c.take_name();
                    if word.is_empty() {
                        return Err(c.err("expected an enum member"));
                    }
                    Value::String(word.to_owned())
                }
            }
        };
        let member_base = match &member {
            Value::String(_) => Some(EnumBase::Str),
            Value::Number(_) => Some(EnumBase::Int),
            _ => None,
        };
        if let Some(mb) = member_base {
            match base {
                None => base = Some(mb),
                Some(b) if b != mb => return Err(c.err("enum members must share one type")),
                Some(_) => {}
            }
        }
        if member == Value::Null && members.contains(&Value::Null) {
            return Err(c.err("enum lists null twice"));
        }
        members.push(member);
        if !c.eat("|") {
            break;
        }
    }
    let Some(base) = base else {
        return Err(c.err("enum needs a non-null member"));
    };
    let nullable = members.contains(&Value::Null);
    Ok(Node {
        kind: Kind::Enum { base, members },
        nullable,
        description: None,
        annot: Annot::default(),
    })
}
