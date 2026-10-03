//! Reversible, readable schema notation. Keyword values remain exact JSON.
use crate::{CompactError, Result, schema, strict_json::StrictValue};
use serde_json::{Map, Value};

fn atom(text: &str) -> String {
    if !text.is_empty()
        && text
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
    {
        text.into()
    } else {
        serde_json::to_string(text).unwrap_or_default()
    }
}
pub(crate) fn encode(schema: &Value) -> String {
    let kind = schema["type"].as_str().unwrap_or("");
    let shorthand = match kind {
        "string" => "str",
        "integer" => "int",
        "number" => "num",
        "boolean" => "bool",
        v => v,
    };
    let mut out = shorthand.to_owned();
    let required = schema.get("required").and_then(Value::as_array);
    let props = schema.get("properties").and_then(Value::as_object);
    // Inline required flags are reversible: print required properties first in
    // the original required-array order, then all remaining properties.
    let inline = required.is_some_and(|req| {
        !req.is_empty()
            && req.iter().all(|f| {
                f.as_str()
                    .is_some_and(|f| props.is_some_and(|p| p.contains_key(f)))
            })
    });
    if let Some(props) = props {
        out.clear();
        out.push('{');
        let mut names: Vec<&str> = if inline {
            required
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect()
        } else {
            Vec::new()
        };
        for key in props.keys() {
            if !names.contains(&key.as_str()) {
                names.push(key);
            }
        }
        for (index, key) in names.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&atom(key));
            if inline && required.is_some_and(|r| r.iter().any(|v| v.as_str() == Some(key))) {
                out.push('!');
            }
            out.push(':');
            out.push_str(&encode(&props[*key]));
        }
        out.push('}');
    }
    if let Some(items) = schema.get("items") {
        out.clear();
        out.push('[');
        out.push_str(&encode(items));
        out.push(']');
    }
    for (key, marker) in [("required", '!'), ("enum", '='), ("description", '~')] {
        if let Some(value) = schema.get(key).filter(|_| key != "required" || !inline) {
            out.push(marker);
            out.push_str(&value.to_string());
        }
    }
    if let Some(format) = schema.get("format").and_then(Value::as_str) {
        out.push('/');
        out.push_str(format);
    }
    let attrs: Map<String, Value> = schema
        .as_object()
        .into_iter()
        .flat_map(|o| o.iter())
        .filter(|(k, _)| {
            ![
                "type",
                "properties",
                "items",
                "required",
                "enum",
                "description",
                "format",
            ]
            .contains(&k.as_str())
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if !attrs.is_empty() {
        out.push('@');
        out.push_str(&Value::Object(attrs).to_string());
    }
    out
}

pub(crate) struct Parser<'a> {
    pub text: &'a str,
    pub pos: usize,
}
impl<'a> Parser<'a> {
    pub fn new(text: &'a str) -> Self {
        Self { text, pos: 0 }
    }
    pub fn space(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }
    fn take(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn need(&mut self, b: u8) -> Result<()> {
        if self.take(b) {
            Ok(())
        } else {
            Err(CompactError::MalformedOutput)
        }
    }
    pub fn atom(&mut self) -> Result<String> {
        if self.peek() == Some(b'"') {
            return self
                .json()?
                .as_str()
                .map(str::to_owned)
                .ok_or(CompactError::MalformedOutput);
        }
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(CompactError::MalformedOutput);
        }
        Ok(self.text[start..self.pos].into())
    }
    pub fn json(&mut self) -> Result<Value> {
        let mut values =
            serde_json::Deserializer::from_str(&self.text[self.pos..]).into_iter::<StrictValue>();
        let v = values
            .next()
            .ok_or(CompactError::MalformedOutput)?
            .map_err(|_| CompactError::MalformedOutput)?
            .0;
        self.pos += values.byte_offset();
        Ok(v)
    }
    pub fn schema(&mut self, depth: usize) -> Result<Value> {
        if depth > 32 {
            return Err(CompactError::ResourceLimit);
        }
        let mut out = Map::new();
        if self.take(b'{') {
            out.insert("type".into(), Value::String("object".into()));
            let mut props = Map::new();
            let mut required = Vec::new();
            if !self.take(b'}') {
                loop {
                    let key = self.atom()?;
                    if self.take(b'!') {
                        required.push(Value::String(key.clone()));
                    }
                    self.need(b':')?;
                    let value = self.schema(depth + 1)?;
                    if props.insert(key, value).is_some() {
                        return Err(CompactError::MalformedOutput);
                    }
                    if self.take(b'}') {
                        break;
                    }
                    self.need(b',')?;
                }
            }
            out.insert("properties".into(), Value::Object(props));
            if !required.is_empty() {
                out.insert("required".into(), Value::Array(required));
            }
        } else if self.take(b'[') {
            out.insert("type".into(), Value::String("array".into()));
            out.insert("items".into(), self.schema(depth + 1)?);
            self.need(b']')?;
        } else {
            let kind = self.atom()?;
            let full = match kind.as_str() {
                "str" => "string",
                "int" => "integer",
                "num" => "number",
                "bool" => "boolean",
                v => v,
            };
            out.insert("type".into(), Value::String(full.into()));
        }
        for (marker, key) in [(b'!', "required"), (b'=', "enum"), (b'~', "description")] {
            if self.take(marker) && out.insert(key.into(), self.json()?).is_some() {
                return Err(CompactError::MalformedOutput);
            }
        }
        if self.take(b'/') {
            out.insert("format".into(), Value::String(self.atom()?));
        }
        if self.take(b'@') {
            let attrs = self.json()?;
            for (k, v) in attrs.as_object().ok_or(CompactError::MalformedOutput)? {
                if out.insert(k.clone(), v.clone()).is_some() {
                    return Err(CompactError::MalformedOutput);
                }
            }
        }
        let result = Value::Object(out);
        schema::check(&result, depth)?;
        Ok(result)
    }
    pub fn end(&self) -> bool {
        self.pos == self.text.len()
    }
}
