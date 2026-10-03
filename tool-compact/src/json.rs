use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

pub fn to_compact_json_value(value: &Value) -> String {
    let sorted = sort_json_value(value);
    serde_json::to_string(&sorted).unwrap_or_else(|_| "{}".to_string())
}

fn sort_json_value(value: &Value) -> Value {
    match value {
        Value::Object(obj) => {
            let mut out = Map::new();
            let mut keys = obj.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            for key in keys {
                let v = obj.get(&key).map(sort_json_value).unwrap_or(Value::Null);
                out.insert(key, v);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_json_value).collect()),
        _ => value.clone(),
    }
}

pub fn parse_strict_json<T>(input: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    let value = parse_strict_value(input)?;
    serde_json::from_value(value).map_err(|err| err.to_string())
}

pub fn parse_strict_value(input: &str) -> Result<Value, String> {
    let trimmed = input.trim();
    let mut parser = JsonParser::new(trimmed);
    let value = parser.parse_value()?;
    parser.skip_ws();
    if !parser.is_eof() {
        return Err("trailing content".to_string());
    }
    Ok(value)
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            pos: 0,
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn parse_value(&mut self) -> Result<Value, String> {
        self.skip_ws();
        if self.is_eof() {
            return Err("unexpected end of JSON".to_string());
        }
        match self.bytes[self.pos] {
            b'{' => self.parse_object(),
            b'[' => self.parse_array(),
            b'"' => self.parse_string().map(Value::String),
            b'-' | b'0'..=b'9' => self.parse_number(),
            b't' => self.parse_literal("true", Value::Bool(true)),
            b'f' => self.parse_literal("false", Value::Bool(false)),
            b'n' => self.parse_literal("null", Value::Null),
            _ => Err(format!(
                "unexpected character {}",
                self.bytes[self.pos] as char
            )),
        }
    }

    fn parse_object(&mut self) -> Result<Value, String> {
        self.pos += 1;
        self.skip_ws();
        let mut map = Map::new();
        if self.is_eof() {
            return Err("unterminated object".to_string());
        }
        if self.bytes[self.pos] == b'}' {
            self.pos += 1;
            return Ok(Value::Object(map));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            if self.pos >= self.bytes.len() || self.bytes[self.pos] != b':' {
                return Err("missing colon in object".to_string());
            }
            self.pos += 1;
            let value = self.parse_value()?;
            if map.contains_key(&key) {
                return Err(format!("duplicate key: {key}"));
            }
            map.insert(key, value);
            self.skip_ws();
            if self.is_eof() {
                return Err("unterminated object".to_string());
            }
            match self.bytes[self.pos] {
                b',' => {
                    self.pos += 1;
                    self.skip_ws();
                    if self.is_eof() {
                        return Err("unterminated object".to_string());
                    }
                    if self.bytes[self.pos] == b'}' {
                        return Err("trailing comma in object".to_string());
                    }
                }
                b'}' => {
                    self.pos += 1;
                    return Ok(Value::Object(map));
                }
                _ => return Err("expected , or } in object".to_string()),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Value, String> {
        self.pos += 1;
        self.skip_ws();
        let mut items = Vec::new();
        if self.is_eof() {
            return Err("unterminated array".to_string());
        }
        if self.bytes[self.pos] == b']' {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            if self.is_eof() {
                return Err("unterminated array".to_string());
            }
            match self.bytes[self.pos] {
                b',' => {
                    self.pos += 1;
                    self.skip_ws();
                    if self.is_eof() {
                        return Err("unterminated array".to_string());
                    }
                    if self.bytes[self.pos] == b']' {
                        return Err("trailing comma in array".to_string());
                    }
                }
                b']' => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err("expected , or ] in array".to_string()),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        if self.is_eof() || self.bytes[self.pos] != b'"' {
            return Err("expected string".to_string());
        }
        self.pos += 1;
        let mut bytes = Vec::new();
        while self.pos < self.bytes.len() {
            let ch = self.bytes[self.pos];
            self.pos += 1;
            match ch {
                b'"' => {
                    return String::from_utf8(bytes)
                        .map_err(|_| "invalid utf-8 in string".to_string());
                }
                b'\\' => {
                    if self.pos >= self.bytes.len() {
                        return Err("unterminated escape".to_string());
                    }
                    let esc = self.bytes[self.pos];
                    self.pos += 1;
                    match esc {
                        b'"' => bytes.push(b'"'),
                        b'\\' => bytes.push(b'\\'),
                        b'/' => bytes.push(b'/'),
                        b'b' => bytes.push(0x08),
                        b'f' => bytes.push(0x0c),
                        b'n' => bytes.push(b'\n'),
                        b'r' => bytes.push(b'\r'),
                        b't' => bytes.push(b'\t'),
                        b'u' => {
                            if self.pos + 4 > self.bytes.len() {
                                return Err("invalid unicode escape".to_string());
                            }
                            let slice = &self.bytes[self.pos..self.pos + 4];
                            let hex = String::from_utf8_lossy(slice).to_string();
                            let value = u16::from_str_radix(&hex, 16)
                                .map_err(|_| "invalid unicode escape".to_string())?;
                            let utf8 = char::from_u32(u32::from(value)).unwrap_or('\u{FFFD}');
                            let mut encoded = [0u8; 4];
                            let encoded_str = utf8.encode_utf8(&mut encoded);
                            bytes.extend_from_slice(encoded_str.as_bytes());
                            self.pos += 4;
                        }
                        _ => return Err(format!("unsupported escape \\{}", esc as char)),
                    }
                }
                _ => bytes.push(ch),
            }
        }
        Err("unterminated string".to_string())
    }

    fn parse_number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        if self.bytes[self.pos] == b'-' {
            self.pos += 1;
        }
        if self.pos >= self.bytes.len() {
            return Err("invalid number".to_string());
        }
        if self.bytes[self.pos] == b'0' {
            self.pos += 1;
        } else {
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'.' {
            self.pos += 1;
            if self.pos >= self.bytes.len() || !self.bytes[self.pos].is_ascii_digit() {
                return Err("invalid number".to_string());
            }
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        if self.pos < self.bytes.len()
            && (self.bytes[self.pos] == b'e' || self.bytes[self.pos] == b'E')
        {
            self.pos += 1;
            if self.pos < self.bytes.len()
                && (self.bytes[self.pos] == b'+' || self.bytes[self.pos] == b'-')
            {
                self.pos += 1;
            }
            if self.pos >= self.bytes.len() || !self.bytes[self.pos].is_ascii_digit() {
                return Err("invalid exponent".to_string());
            }
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        let s = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or_default();
        if s.contains('.') || s.contains('e') || s.contains('E') {
            Ok(Value::from(
                s.parse::<f64>().map_err(|_| "invalid number".to_string())?,
            ))
        } else {
            Ok(Value::from(
                s.parse::<i64>().map_err(|_| "invalid number".to_string())?,
            ))
        }
    }

    fn parse_literal(&mut self, literal: &str, value: Value) -> Result<Value, String> {
        let end = self.pos + literal.len();
        if end <= self.bytes.len() && &self.bytes[self.pos..end] == literal.as_bytes() {
            self.pos = end;
            Ok(value)
        } else {
            Err(format!("expected {literal}"))
        }
    }
}
