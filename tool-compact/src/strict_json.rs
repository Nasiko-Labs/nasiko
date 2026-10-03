use std::collections::HashSet;
use serde_json::{Map, Number, Value};
use crate::error::CompactError;
use crate::limits::Limits;

/// Strict RFC 8259 JSON parser that rejects duplicate object keys after string decoding
/// and enforces container depth and argument size limits.
pub struct StrictJsonParser<'a> {
    input: &'a str,
    bytes: &'a [u8],
    pos: usize,
    max_depth: usize,
    current_depth: usize,
}

impl<'a> StrictJsonParser<'a> {
    pub fn new(input: &'a str, limits: &Limits) -> Self {
        Self {
            input,
            bytes: input.as_bytes(),
            pos: 0,
            max_depth: limits.max_json_depth,
            current_depth: 0,
        }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn parse_object(&mut self) -> Result<(Value, usize), CompactError> {
        self.skip_whitespace();
        let start_pos = self.pos;
        if self.pos >= self.bytes.len() || self.bytes[self.pos] != b'{' {
            return Err(CompactError::InvalidJson {
                reason: "expected '{' at start of JSON object".to_string(),
                offset: self.pos,
            });
        }

        let val = self.parse_value()?;
        self.skip_whitespace();
        let end_pos = self.pos;

        if !val.is_object() {
            return Err(CompactError::InvalidJson {
                reason: "root argument type must be an object".to_string(),
                offset: start_pos,
            });
        }

        Ok((val, end_pos))
    }

    pub fn parse_value(&mut self) -> Result<Value, CompactError> {
        self.skip_whitespace();
        if self.pos >= self.bytes.len() {
            return Err(CompactError::IncompleteCall {
                reason: "unexpected end of input while parsing JSON".to_string(),
                offset: self.pos,
            });
        }

        match self.bytes[self.pos] {
            b'{' => self.parse_obj(),
            b'[' => self.parse_arr(),
            b'"' => self.parse_str().map(Value::String),
            b't' | b'f' => self.parse_bool(),
            b'n' => self.parse_null(),
            b'-' | b'0'..=b'9' => self.parse_num(),
            b => Err(CompactError::InvalidJson {
                reason: format!("unexpected character '{}' in JSON", b as char),
                offset: self.pos,
            }),
        }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn parse_obj(&mut self) -> Result<Value, CompactError> {
        let obj_start = self.pos;
        self.pos += 1; // skip '{'
        self.current_depth += 1;
        if self.current_depth > self.max_depth {
            return Err(CompactError::LimitExceeded {
                limit: "max_json_depth",
                value: self.current_depth,
                max: self.max_depth,
            });
        }

        let mut map = Map::new();
        let mut seen_keys = HashSet::new();

        self.skip_whitespace();
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'}' {
            self.pos += 1;
            self.current_depth -= 1;
            return Ok(Value::Object(map));
        }

        loop {
            self.skip_whitespace();
            if self.pos >= self.bytes.len() {
                return Err(CompactError::IncompleteCall {
                    reason: "unterminated object".to_string(),
                    offset: obj_start,
                });
            }

            if self.bytes[self.pos] != b'"' {
                return Err(CompactError::InvalidJson {
                    reason: "expected string key in object".to_string(),
                    offset: self.pos,
                });
            }

            let key_offset = self.pos;
            let key = self.parse_str()?;

            // Strict duplicate-key detection after string decoding:
            // "a" and "\u0061" decode to the same key string and must be rejected
            if !seen_keys.insert(key.clone()) {
                return Err(CompactError::DuplicateKey {
                    key,
                    offset: key_offset,
                });
            }

            self.skip_whitespace();
            if self.pos >= self.bytes.len() || self.bytes[self.pos] != b':' {
                return Err(CompactError::InvalidJson {
                    reason: "expected ':' after key".to_string(),
                    offset: self.pos,
                });
            }
            self.pos += 1; // skip ':'

            let value = self.parse_value()?;
            map.insert(key, value);

            self.skip_whitespace();
            if self.pos >= self.bytes.len() {
                return Err(CompactError::IncompleteCall {
                    reason: "unterminated object".to_string(),
                    offset: obj_start,
                });
            }

            if self.bytes[self.pos] == b',' {
                self.pos += 1;
                self.skip_whitespace();
                // Reject trailing comma
                if self.pos < self.bytes.len() && self.bytes[self.pos] == b'}' {
                    return Err(CompactError::InvalidJson {
                        reason: "trailing comma in object".to_string(),
                        offset: self.pos,
                    });
                }
            } else if self.bytes[self.pos] == b'}' {
                self.pos += 1;
                break;
            } else {
                return Err(CompactError::InvalidJson {
                    reason: "expected ',' or '}' in object".to_string(),
                    offset: self.pos,
                });
            }
        }

        self.current_depth -= 1;
        Ok(Value::Object(map))
    }

    fn parse_arr(&mut self) -> Result<Value, CompactError> {
        let arr_start = self.pos;
        self.pos += 1; // skip '['
        self.current_depth += 1;
        if self.current_depth > self.max_depth {
            return Err(CompactError::LimitExceeded {
                limit: "max_json_depth",
                value: self.current_depth,
                max: self.max_depth,
            });
        }

        let mut list = Vec::new();
        self.skip_whitespace();
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b']' {
            self.pos += 1;
            self.current_depth -= 1;
            return Ok(Value::Array(list));
        }

        loop {
            let item = self.parse_value()?;
            list.push(item);

            self.skip_whitespace();
            if self.pos >= self.bytes.len() {
                return Err(CompactError::IncompleteCall {
                    reason: "unterminated array".to_string(),
                    offset: arr_start,
                });
            }

            if self.bytes[self.pos] == b',' {
                self.pos += 1;
                self.skip_whitespace();
                // Reject trailing comma
                if self.pos < self.bytes.len() && self.bytes[self.pos] == b']' {
                    return Err(CompactError::InvalidJson {
                        reason: "trailing comma in array".to_string(),
                        offset: self.pos,
                    });
                }
            } else if self.bytes[self.pos] == b']' {
                self.pos += 1;
                break;
            } else {
                return Err(CompactError::InvalidJson {
                    reason: "expected ',' or ']' in array".to_string(),
                    offset: self.pos,
                });
            }
        }

        self.current_depth -= 1;
        Ok(Value::Array(list))
    }

    fn parse_str(&mut self) -> Result<String, CompactError> {
        let start_offset = self.pos;
        self.pos += 1; // skip initial '"'
        let mut out = String::new();

        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'"' {
                self.pos += 1;
                return Ok(out);
            }
            if b == b'\\' {
                self.pos += 1;
                if self.pos >= self.bytes.len() {
                    return Err(CompactError::IncompleteCall {
                        reason: "incomplete escape sequence in string".to_string(),
                        offset: start_offset,
                    });
                }
                match self.bytes[self.pos] {
                    b'"' => { out.push('"'); self.pos += 1; }
                    b'\\' => { out.push('\\'); self.pos += 1; }
                    b'/' => { out.push('/'); self.pos += 1; }
                    b'b' => { out.push('\x08'); self.pos += 1; }
                    b'f' => { out.push('\x0C'); self.pos += 1; }
                    b'n' => { out.push('\n'); self.pos += 1; }
                    b'r' => { out.push('\r'); self.pos += 1; }
                    b't' => { out.push('\t'); self.pos += 1; }
                    b'u' => {
                        self.pos += 1;
                        let codepoint = self.parse_hex4()?;
                        if (0xD800..=0xDBFF).contains(&codepoint) {
                            // High surrogate, require low surrogate \uDC00..\uDFFF
                            if self.pos + 2 < self.bytes.len()
                                && self.bytes[self.pos] == b'\\'
                                && self.bytes[self.pos + 1] == b'u'
                            {
                                self.pos += 2;
                                let low = self.parse_hex4()?;
                                if (0xDC00..=0xDFFF).contains(&low) {
                                    let combined = 0x10000 + (((codepoint - 0xD800) << 10) | (low - 0xDC00));
                                    if let Some(ch) = char::from_u32(combined) {
                                        out.push(ch);
                                    } else {
                                        return Err(CompactError::InvalidJson {
                                            reason: format!("invalid surrogate pair \\u{:04x}\\u{:04x}", codepoint, low),
                                            offset: self.pos - 6,
                                        });
                                    }
                                } else {
                                    return Err(CompactError::InvalidJson {
                                        reason: format!("expected low surrogate, found \\u{:04x}", low),
                                        offset: self.pos - 6,
                                    });
                                }
                            } else {
                                return Err(CompactError::InvalidJson {
                                    reason: "unpaired high surrogate".to_string(),
                                    offset: self.pos - 4,
                                });
                            }
                        } else if (0xDC00..=0xDFFF).contains(&codepoint) {
                            return Err(CompactError::InvalidJson {
                                reason: "unpaired low surrogate".to_string(),
                                offset: self.pos - 4,
                            });
                        } else if let Some(ch) = char::from_u32(codepoint) {
                            out.push(ch);
                        } else {
                            return Err(CompactError::InvalidJson {
                                reason: format!("invalid unicode escape \\u{:04x}", codepoint),
                                offset: self.pos - 4,
                            });
                        }
                    }
                    invalid => {
                        return Err(CompactError::InvalidJson {
                            reason: format!("invalid escape sequence '\\{}'", invalid as char),
                            offset: self.pos - 1,
                        });
                    }
                }
            } else if b < 0x20 {
                return Err(CompactError::InvalidJson {
                    reason: format!("unescaped control character (0x{:02x}) in string", b),
                    offset: self.pos,
                });
            } else {
                // Decode UTF-8 sequence
                let rem = &self.input[self.pos..];
                let ch = rem.chars().next().ok_or_else(|| CompactError::IncompleteCall {
                    reason: "unexpected end of string".to_string(),
                    offset: self.pos,
                })?;
                out.push(ch);
                self.pos += ch.len_utf8();
            }
        }

        Err(CompactError::IncompleteCall {
            reason: "unterminated string".to_string(),
            offset: start_offset,
        })
    }

    fn parse_hex4(&mut self) -> Result<u32, CompactError> {
        if self.pos + 4 > self.bytes.len() {
            return Err(CompactError::IncompleteCall {
                reason: "incomplete \\u hex escape".to_string(),
                offset: self.pos,
            });
        }
        let hex_str = &self.input[self.pos..self.pos + 4];
        let val = u32::from_str_radix(hex_str, 16).map_err(|_| CompactError::InvalidJson {
            reason: format!("invalid hex digits in \\u escape: '{}'", hex_str),
            offset: self.pos,
        })?;
        self.pos += 4;
        Ok(val)
    }

    fn parse_bool(&mut self) -> Result<Value, CompactError> {
        if self.input[self.pos..].starts_with("true") {
            self.pos += 4;
            Ok(Value::Bool(true))
        } else if self.input[self.pos..].starts_with("false") {
            self.pos += 5;
            Ok(Value::Bool(false))
        } else {
            Err(CompactError::InvalidJson {
                reason: "invalid boolean literal".to_string(),
                offset: self.pos,
            })
        }
    }

    fn parse_null(&mut self) -> Result<Value, CompactError> {
        if self.input[self.pos..].starts_with("null") {
            self.pos += 4;
            Ok(Value::Null)
        } else {
            Err(CompactError::InvalidJson {
                reason: "invalid null literal".to_string(),
                offset: self.pos,
            })
        }
    }

    fn parse_num(&mut self) -> Result<Value, CompactError> {
        let start = self.pos;
        if self.bytes[self.pos] == b'-' {
            self.pos += 1;
        }

        if self.pos >= self.bytes.len() {
            return Err(CompactError::IncompleteCall {
                reason: "incomplete number".to_string(),
                offset: start,
            });
        }

        if self.bytes[self.pos] == b'0' {
            self.pos += 1;
        } else if self.bytes[self.pos].is_ascii_digit() {
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        } else {
            return Err(CompactError::InvalidJson {
                reason: "invalid number".to_string(),
                offset: self.pos,
            });
        }

        // Fraction
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'.' {
            self.pos += 1;
            if self.pos >= self.bytes.len() || !self.bytes[self.pos].is_ascii_digit() {
                return Err(CompactError::InvalidJson {
                    reason: "expected digit after decimal point".to_string(),
                    offset: self.pos,
                });
            }
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }

        // Exponent
        if self.pos < self.bytes.len() && (self.bytes[self.pos] == b'e' || self.bytes[self.pos] == b'E') {
            self.pos += 1;
            if self.pos < self.bytes.len() && (self.bytes[self.pos] == b'+' || self.bytes[self.pos] == b'-') {
                self.pos += 1;
            }
            if self.pos >= self.bytes.len() || !self.bytes[self.pos].is_ascii_digit() {
                return Err(CompactError::InvalidJson {
                    reason: "expected digit in exponent".to_string(),
                    offset: self.pos,
                });
            }
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }

        let num_str = &self.input[start..self.pos];
        let n: Number = serde_json::from_str(num_str).map_err(|_| CompactError::NumericRange {
            reason: format!("number '{}' exceeds numeric range", num_str),
            offset: start,
        })?;

        Ok(Value::Number(n))
    }
}
