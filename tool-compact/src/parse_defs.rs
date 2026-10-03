//! Compact definitions text → [`Ty`]. The inverse of `render`, used by `decode_tools`.

use serde_json::{Number, Value};

use crate::render::KEYWORDS;
use crate::ty::{Bounds, Field, Format, Obj, Ty, is_valid_name};

pub(crate) struct ParsedTool {
    pub name: String,
    pub desc: Option<String>,
    pub params: Obj,
}

pub(crate) fn parse_definitions(text: &str) -> Result<Vec<ParsedTool>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(n, line)| parse_tool(line).map_err(|e| format!("line {}: {e}", n + 1)))
        .collect()
}

fn parse_tool(line: &str) -> Result<ParsedTool, String> {
    let mut p = Parser { s: line, i: 0 };
    let name = p.name()?;
    p.expect("(")?;
    let params = p.params(')')?;
    let desc = match p.rest() {
        "" => None,
        rest => Some(
            rest.strip_prefix(" - ")
                .ok_or_else(|| format!("unexpected `{rest}` after `{name}(...)`"))?
                .to_string(),
        ),
    };
    Ok(ParsedTool {
        name: name.to_string(),
        desc,
        params,
    })
}

/// One recursive-descent parser over a single signature line.
struct Parser<'a> {
    s: &'a str,
    i: usize,
}

/// A parsed `|` alternative: an enum literal or a type.
enum Alt {
    Lit(Value),
    Ty(Ty),
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }

    fn eat(&mut self, token: &str) -> bool {
        let found = self.rest().starts_with(token);
        if found {
            self.i += token.len();
        }
        found
    }

    fn expect(&mut self, token: &str) -> Result<(), String> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(format!("expected `{token}` at `{}`", self.rest()))
        }
    }

    fn name(&mut self) -> Result<&'a str, String> {
        let rest = self.rest();
        let len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
            .unwrap_or(rest.len());
        let name = &rest[..len];
        if !is_valid_name(name) {
            return Err(format!("expected a name at `{rest}`"));
        }
        self.i += len;
        Ok(name)
    }

    /// `param (", " param)*` up to and including `close`.
    fn params(&mut self, close: char) -> Result<Obj, String> {
        let mut obj = Obj {
            fields: Vec::new(),
            open: false,
        };
        let close_str = close.to_string();
        if self.eat(&close_str) {
            return Ok(obj);
        }
        loop {
            if self.eat("...") {
                obj.open = true;
            } else {
                obj.fields.push(self.field()?);
            }
            if self.eat(&close_str) {
                return Ok(obj);
            }
            self.expect(", ")?;
        }
    }

    fn field(&mut self) -> Result<Field, String> {
        let name = self.name()?.to_string();
        let required = !self.eat("?");
        self.expect(":")?;
        let ty = self.ty()?;
        let default = if self.eat("=") {
            Some(self.default_value()?)
        } else {
            None
        };
        let desc = if self.rest().starts_with(" (") || self.rest().starts_with(" \"") {
            self.i += 1;
            Some(self.desc()?)
        } else {
            None
        };
        Ok(Field {
            name,
            ty,
            required,
            desc,
            default,
        })
    }

    /// A default: JSON (`10`, `"a b"`, `[1]`, `{"k":1}`, `true`, `null`) or a bare word.
    fn default_value(&mut self) -> Result<Value, String> {
        let rest = self.rest();
        if rest.starts_with(['[', '{']) {
            // Arrays and objects are self-delimiting, so the stream reader stops exactly at
            // their end.
            let mut values = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
            return match values.next() {
                Some(Ok(v)) => {
                    self.i += values.byte_offset();
                    Ok(v)
                }
                _ => Err(format!("invalid default at `{rest}`")),
            };
        }
        if rest.starts_with(|c: char| c == '"' || c == '-' || c.is_ascii_digit()) {
            return self.literal();
        }
        Ok(match self.name()? {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            "null" => Value::Null,
            word => Value::String(word.to_string()),
        })
    }

    /// `(balanced text)` or a JSON string.
    fn desc(&mut self) -> Result<String, String> {
        if self.rest().starts_with('"') {
            return match self.literal()? {
                Value::String(s) => Ok(s),
                _ => Err("description must be a string".into()),
            };
        }
        self.expect("(")?;
        let mut depth = 1;
        for (j, c) in self.rest().char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        let text = self.rest()[..j].to_string();
                        self.i += j + 1;
                        return Ok(text);
                    }
                }
                _ => {}
            }
        }
        Err("unclosed description".into())
    }

    /// `alt ("|" alt)*`: an enum of literals, `T|null`, or a single type.
    fn ty(&mut self) -> Result<Ty, String> {
        let mut alts = vec![self.alt()?];
        while self.eat("|") {
            alts.push(self.alt()?);
        }
        if alts.iter().all(|a| matches!(a, Alt::Lit(_))) {
            return Ok(Ty::Enum(
                alts.into_iter()
                    .filter_map(|a| match a {
                        Alt::Lit(v) => Some(v),
                        Alt::Ty(_) => None,
                    })
                    .collect(),
            ));
        }
        let mut tys = alts.into_iter().map(|a| match a {
            Alt::Lit(v) => Ty::Enum(vec![v]),
            Alt::Ty(t) => t,
        });
        match (tys.next(), tys.next(), tys.next()) {
            (Some(t), None, None) => Ok(t),
            (Some(t), Some(Ty::Null), None) => Ok(Ty::Nullable(Box::new(t))),
            _ => Err("only enums and `T|null` may use `|`".into()),
        }
    }

    /// `atom [bounds] ("[]" [bounds])*`
    fn alt(&mut self) -> Result<Alt, String> {
        let atom = self.atom()?;
        let mut alt = self.maybe_bounds(atom)?;
        while self.eat("[]") {
            let inner = match alt {
                Alt::Lit(v) => Ty::Enum(vec![v]),
                Alt::Ty(t) => t,
            };
            alt = self.maybe_bounds(Alt::Ty(Ty::Arr(Box::new(inner))))?;
        }
        Ok(alt)
    }

    /// Limits written straight after a number, string or array type. (A `(` here can only be
    /// limits: grouping parentheses open an atom, and a description follows a space.)
    fn maybe_bounds(&mut self, alt: Alt) -> Result<Alt, String> {
        if !self.rest().starts_with('(') {
            return Ok(alt);
        }
        let ty = match alt {
            Alt::Ty(ty @ (Ty::Int | Ty::Num | Ty::Str(_) | Ty::Arr(_))) => ty,
            _ => return Err(format!("limits are not valid at `{}`", self.rest())),
        };
        let b = self.bounds()?;
        let counts_only = matches!(ty, Ty::Str(_) | Ty::Arr(_));
        let whole = |n: &Option<Number>| n.as_ref().is_none_or(Number::is_u64);
        if counts_only && (b.min_exclusive || b.max_exclusive || !whole(&b.min) || !whole(&b.max)) {
            return Err("length limits must be whole, inclusive numbers".into());
        }
        Ok(Alt::Ty(Ty::Bounded(Box::new(ty), b)))
    }

    /// `(lo..hi)`, either side optional; `>` / `<` mark an exclusive limit.
    fn bounds(&mut self) -> Result<Bounds, String> {
        self.expect("(")?;
        let min_exclusive = self.eat(">");
        let min = self.bound_number()?;
        self.expect("..")?;
        let max_exclusive = self.eat("<");
        let max = self.bound_number()?;
        self.expect(")")?;
        if (min.is_none() && max.is_none())
            || (min_exclusive && min.is_none())
            || (max_exclusive && max.is_none())
        {
            return Err("empty limits".into());
        }
        Ok(Bounds {
            min,
            max,
            min_exclusive,
            max_exclusive,
        })
    }

    /// A JSON number, stopping before `..` (so `1..480` reads as `1`).
    fn bound_number(&mut self) -> Result<Option<Number>, String> {
        let rest = self.rest();
        let b = rest.as_bytes();
        let mut len = 0;
        while let Some(&c) = b.get(len) {
            let part_of_number = c.is_ascii_digit()
                || matches!(c, b'-' | b'+' | b'e' | b'E')
                || (c == b'.' && b.get(len + 1).is_some_and(u8::is_ascii_digit));
            if !part_of_number {
                break;
            }
            len += 1;
        }
        if len == 0 {
            return Ok(None);
        }
        let n = serde_json::from_str::<Number>(&rest[..len])
            .map_err(|_| format!("invalid limit `{}`", &rest[..len]))?;
        self.i += len;
        Ok(Some(n))
    }

    fn atom(&mut self) -> Result<Alt, String> {
        if self.eat("{") {
            return Ok(Alt::Ty(Ty::Obj(self.params('}')?)));
        }
        if self.eat("(") {
            let ty = self.ty()?;
            self.expect(")")?;
            return Ok(Alt::Ty(ty));
        }
        if self
            .rest()
            .starts_with(|c: char| c == '"' || c == '-' || c.is_ascii_digit())
        {
            return Ok(Alt::Lit(self.literal()?));
        }
        let word = self.name()?;
        if !KEYWORDS.contains(&word) {
            return Ok(Alt::Lit(Value::String(word.to_string())));
        }
        Ok(Alt::Ty(match word {
            "any" => Ty::Any,
            "string" => Ty::Str(None),
            "datetime" => Ty::Str(Some(Format::DateTime)),
            "date" => Ty::Str(Some(Format::Date)),
            "email" => Ty::Str(Some(Format::Email)),
            "uri" => Ty::Str(Some(Format::Uri)),
            "int" => Ty::Int,
            "number" => Ty::Num,
            "bool" => Ty::Bool,
            "null" => Ty::Null,
            _ => Ty::Obj(Obj {
                fields: Vec::new(),
                open: true,
            }),
        }))
    }

    /// One JSON string or number, consumed exactly. (serde_json's stream reader refuses a number
    /// followed by `|` or `)`, so the token is delimited here and parsed on its own.)
    fn literal(&mut self) -> Result<Value, String> {
        let rest = self.rest();
        let len = if rest.starts_with('"') {
            let mut escaped = false;
            rest.char_indices()
                .skip(1)
                .find(|&(_, c)| {
                    let closes = !escaped && c == '"';
                    escaped = !escaped && c == '\\';
                    closes
                })
                .map(|(j, _)| j + 1)
                .ok_or("unclosed string")?
        } else {
            rest.find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E')))
                .unwrap_or(rest.len())
        };
        match serde_json::from_str::<Value>(&rest[..len]) {
            Ok(v) if v.is_string() || v.is_number() => {
                self.i += len;
                Ok(v)
            }
            _ => Err(format!("invalid literal at `{rest}`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ty(s: &str) -> Ty {
        let mut p = Parser { s, i: 0 };
        let t = p.ty().unwrap();
        assert_eq!(p.rest(), "", "unparsed tail in {s:?}");
        t
    }

    #[test]
    fn parses_types() {
        assert_eq!(ty("string[]"), Ty::Arr(Box::new(Ty::Str(None))));
        assert_eq!(
            ty("public|private"),
            Ty::Enum(vec![json!("public"), json!("private")])
        );
        assert_eq!(
            ty("(\"a|b\"|c)[]"),
            Ty::Arr(Box::new(Ty::Enum(vec![json!("a|b"), json!("c")])))
        );
        assert_eq!(ty("int|null"), Ty::Nullable(Box::new(Ty::Int)));
        assert_eq!(ty("1|-2.5"), Ty::Enum(vec![json!(1), json!(-2.5)]));
        assert_eq!(ty("\"null\""), Ty::Enum(vec![json!("null")]));
        assert_eq!(
            ty("number(>-1.5..<1e3)[](1..)"),
            Ty::Bounded(
                Box::new(Ty::Arr(Box::new(Ty::Bounded(
                    Box::new(Ty::Num),
                    Bounds {
                        min: Some(serde_json::from_str("-1.5").unwrap()),
                        max: Some(serde_json::from_str("1e3").unwrap()),
                        min_exclusive: true,
                        max_exclusive: true,
                    }
                )))),
                Bounds {
                    min: Some(1.into()),
                    max: None,
                    min_exclusive: false,
                    max_exclusive: false
                }
            )
        );
        assert_eq!(
            ty("({a:int}|null)[]"),
            Ty::Arr(Box::new(Ty::Nullable(Box::new(Ty::Obj(Obj {
                fields: vec![Field {
                    name: "a".into(),
                    ty: Ty::Int,
                    required: true,
                    desc: None,
                    default: None,
                }],
                open: false,
            })))))
        );
    }

    #[test]
    fn parses_a_tool_line() {
        let t = parse_tool(
            "send(to:email[] (Who, (exactly)), note?:string \"a ) b\", ...) - Send it - now.",
        )
        .unwrap();
        assert_eq!(t.name, "send");
        assert_eq!(t.desc.as_deref(), Some("Send it - now."));
        assert!(t.params.open);
        assert_eq!(t.params.fields[0].desc.as_deref(), Some("Who, (exactly)"));
        assert_eq!(t.params.fields[1].desc.as_deref(), Some("a ) b"));
        assert!(!t.params.fields[1].required);
    }

    #[test]
    fn rejects_malformed_lines() {
        for bad in [
            "t(a:int",
            "t(a int)",
            "t(a:int,b:int)",
            "t(a:string|int)",
            "t(a:int (open)",
            "t(a:int) trailing",
            "t(a:int())",
            "t(a:string(>1..))",
            "t(a:bool(1..2))",
            "t(a:int=)",
            "bad name(a:int)",
        ] {
            assert!(parse_tool(bad).is_err(), "accepted {bad:?}");
        }
    }
}
