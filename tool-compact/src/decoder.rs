use serde_json::{Map, Number, Value};
use crate::error::{Result, ToolCompactError};

/// Decodes a compact tool call string into a tuple of `(tool_name, serde_json::Value)`.
///
/// Example input:
/// `weather_get(location="New York", units="celsius")`
pub fn decode_call(input: &str) -> Result<(String, Value)> {
    let mut parser = Parser::new(input);
    let (tool_name, args) = parser.parse_tool_call()?;
    parser.skip_whitespace();
    if !parser.is_eof() {
        return Err(ToolCompactError::Malformed(format!(
            "Unexpected content after tool call end: '{}'",
            parser.remaining()
        )));
    }
    Ok((tool_name, args))
}

pub(crate) struct Parser<'a> {
    _input: &'a str,
    chars: Vec<char>,
    pos: usize,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            _input: input,
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    pub fn is_eof(&self) -> bool {
        self.pos >= self.chars.len()
    }

    pub fn peek(&self) -> Option<char> {
        if self.pos < self.chars.len() {
            Some(self.chars[self.pos])
        } else {
            None
        }
    }

    pub fn advance(&mut self) -> Option<char> {
        if self.pos < self.chars.len() {
            let ch = self.chars[self.pos];
            self.pos += 1;
            Some(ch)
        } else {
            None
        }
    }

    pub fn remaining(&self) -> String {
        self.chars[self.pos..].iter().collect()
    }

    pub fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    pub fn parse_tool_call(&mut self) -> Result<(String, Value)> {
        self.skip_whitespace();
        let tool_name = self.parse_identifier()?;
        self.skip_whitespace();

        if self.peek() != Some('(') {
            return Err(ToolCompactError::Malformed(format!(
                "Expected '(' after tool name '{}', found '{:?}'",
                tool_name,
                self.peek()
            )));
        }
        self.advance(); // consume '('

        let mut map = Map::new();
        self.skip_whitespace();

        if self.peek() == Some(')') {
            self.advance(); // consume ')'
            return Ok((tool_name, Value::Object(map)));
        }

        loop {
            self.skip_whitespace();
            if self.is_eof() {
                return Err(ToolCompactError::Malformed(
                    "Unexpected end of input while parsing argument list".into(),
                ));
            }

            let key = self.parse_identifier()?;
            self.skip_whitespace();

            if self.peek() != Some('=') && self.peek() != Some(':') {
                return Err(ToolCompactError::Malformed(format!(
                    "Expected '=' or ':' after argument key '{}', found '{:?}'",
                    key,
                    self.peek()
                )));
            }
            self.advance(); // consume '=' or ':'

            self.skip_whitespace();
            let val = self.parse_value()?;
            map.insert(key, val);

            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.advance(); // consume ','
                }
                Some(')') => {
                    self.advance(); // consume ')'
                    break;
                }
                Some(ch) => {
                    return Err(ToolCompactError::Malformed(format!(
                        "Expected ',' or ')' after argument value, found '{}'",
                        ch
                    )));
                }
                None => {
                    return Err(ToolCompactError::Malformed(
                        "Unclosed argument list, missing ')'".into(),
                    ));
                }
            }
        }

        Ok((tool_name, Value::Object(map)))
    }

    pub fn parse_identifier(&mut self) -> Result<String> {
        self.skip_whitespace();
        let mut id = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                id.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        if id.is_empty() {
            return Err(ToolCompactError::Malformed(format!(
                "Expected identifier, found '{:?}'",
                self.peek()
            )));
        }
        Ok(id)
    }

    pub fn parse_value(&mut self) -> Result<Value> {
        self.skip_whitespace();
        let ch = match self.peek() {
            Some(c) => c,
            None => {
                return Err(ToolCompactError::Malformed(
                    "Unexpected end of input while parsing value".into(),
                ));
            }
        };

        if ch == '"' || ch == '\'' {
            return self.parse_string();
        } else if ch == '[' {
            return self.parse_array();
        } else if ch == '{' {
            return self.parse_object();
        } else if ch == '-' || ch.is_ascii_digit() {
            return self.parse_number();
        } else if ch.is_alphabetic() {
            let ident = self.parse_identifier()?;
            match ident.as_str() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                "null" => Ok(Value::Null),
                other => Ok(Value::String(other.to_string())),
            }
        } else {
            Err(ToolCompactError::Malformed(format!(
                "Unexpected character starting value: '{}'",
                ch
            )))
        }
    }

    pub fn parse_string(&mut self) -> Result<Value> {
        let quote = self.advance().unwrap(); // consume '"' or '\''
        let mut s = String::new();
        let mut escaped = false;

        while let Some(ch) = self.advance() {
            if escaped {
                match ch {
                    '"' => s.push('"'),
                    '\'' => s.push('\''),
                    '\\' => s.push('\\'),
                    '/' => s.push('/'),
                    'n' => s.push('\n'),
                    'r' => s.push('\r'),
                    't' => s.push('\t'),
                    'u' => {
                        let mut hex = String::new();
                        for _ in 0..4 {
                            if let Some(hc) = self.advance() {
                                hex.push(hc);
                            } else {
                                return Err(ToolCompactError::Malformed(
                                    "Incomplete unicode escape sequence".into(),
                                ));
                            }
                        }
                        if let Ok(code) = u32::from_str_radix(&hex, 16) {
                            if let Some(unicode_char) = std::char::from_u32(code) {
                                s.push(unicode_char);
                            } else {
                                return Err(ToolCompactError::Malformed(format!(
                                    "Invalid unicode character code: {}",
                                    hex
                                )));
                            }
                        } else {
                            return Err(ToolCompactError::Malformed(format!(
                                "Invalid hex in unicode escape sequence: {}",
                                hex
                            )));
                        }
                    }
                    other => s.push(other),
                }
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                return Ok(Value::String(s));
            } else {
                s.push(ch);
            }
        }

        Err(ToolCompactError::Malformed(
            "Unclosed string literal".into(),
        ))
    }

    pub fn parse_number(&mut self) -> Result<Value> {
        let mut num_str = String::new();
        if self.peek() == Some('-') {
            num_str.push(self.advance().unwrap());
        }
        let mut is_float = false;

        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                num_str.push(ch);
                self.advance();
            } else if ch == '.' && !is_float {
                is_float = true;
                num_str.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        if is_float {
            let f: f64 = num_str.parse().map_err(|_| {
                ToolCompactError::Malformed(format!("Invalid float number: '{}'", num_str))
            })?;
            Ok(Value::Number(
                Number::from_f64(f).ok_or_else(|| {
                    ToolCompactError::Malformed(format!("Invalid float value: '{}'", num_str))
                })?,
            ))
        } else {
            let i: i64 = num_str.parse().map_err(|_| {
                ToolCompactError::Malformed(format!("Invalid integer number: '{}'", num_str))
            })?;
            Ok(Value::Number(Number::from(i)))
        }
    }

    pub fn parse_array(&mut self) -> Result<Value> {
        self.advance(); // consume '['
        let mut list = Vec::new();
        self.skip_whitespace();

        if self.peek() == Some(']') {
            self.advance(); // consume ']'
            return Ok(Value::Array(list));
        }

        loop {
            self.skip_whitespace();
            let elem = self.parse_value()?;
            list.push(elem);

            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.advance();
                }
                Some(']') => {
                    self.advance();
                    break;
                }
                Some(ch) => {
                    return Err(ToolCompactError::Malformed(format!(
                        "Expected ',' or ']' in array, found '{}'",
                        ch
                    )));
                }
                None => {
                    return Err(ToolCompactError::Malformed(
                        "Unclosed array literal, missing ']'".into(),
                    ));
                }
            }
        }

        Ok(Value::Array(list))
    }

    pub fn parse_object(&mut self) -> Result<Value> {
        self.advance(); // consume '{'
        let mut map = Map::new();
        self.skip_whitespace();

        if self.peek() == Some('}') {
            self.advance(); // consume '}'
            return Ok(Value::Object(map));
        }

        loop {
            self.skip_whitespace();
            let key = if self.peek() == Some('"') || self.peek() == Some('\'') {
                if let Value::String(s) = self.parse_string()? {
                    s
                } else {
                    unreachable!()
                }
            } else {
                self.parse_identifier()?
            };

            self.skip_whitespace();
            if self.peek() != Some(':') && self.peek() != Some('=') {
                return Err(ToolCompactError::Malformed(format!(
                    "Expected ':' after object key '{}', found '{:?}'",
                    key,
                    self.peek()
                )));
            }
            self.advance();

            self.skip_whitespace();
            let val = self.parse_value()?;
            map.insert(key, val);

            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.advance();
                }
                Some('}') => {
                    self.advance();
                    break;
                }
                Some(ch) => {
                    return Err(ToolCompactError::Malformed(format!(
                        "Expected ',' or '}}' in object, found '{}'",
                        ch
                    )));
                }
                None => {
                    return Err(ToolCompactError::Malformed(
                        "Unclosed object literal, missing '}'".into(),
                    ));
                }
            }
        }

        Ok(Value::Object(map))
    }
}
