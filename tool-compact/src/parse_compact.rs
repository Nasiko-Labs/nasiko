//! Strict parser for compact tool definitions back into canonical schemas and ToolDefs.

use crate::error::Error;
use crate::schema::{Field, StrFormat, ToolSchema, Ty, collapse_whitespace, is_valid_ident};
use crate::types::{CompactTools, ToolDef};

/// Parse compact prompt definitions back into a vector of canonical ToolSchemas.
pub fn parse_compact_definitions(text: &str) -> Result<Vec<ToolSchema>, Error> {
    let mut schemas = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let schema = parse_tool_line(trimmed)?;
        schemas.push(schema);
    }
    Ok(schemas)
}

/// Decode CompactTools into ToolDefs using canonical schema parsing.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, Error> {
    let schemas = parse_compact_definitions(&compact.definitions)?;
    Ok(schemas.iter().map(|s| s.to_tool_def()).collect())
}

fn parse_tool_line(line: &str) -> Result<ToolSchema, Error> {
    let mut p = Parser::new(line);
    let name = p.parse_ident()?;
    if !is_valid_ident(&name) {
        return Err(Error::MalformedCall(format!("invalid tool name '{name}'")));
    }

    p.skip_whitespace();
    p.expect_char('(')?;

    let mut fields = Vec::new();
    p.skip_whitespace();
    if p.peek() != Some(')') {
        loop {
            p.skip_whitespace();
            let field = p.parse_field()?;
            fields.push(field);
            p.skip_whitespace();
            if p.peek() == Some(',') {
                p.bump();
                p.skip_whitespace();
            } else {
                break;
            }
        }
    }

    p.expect_char(')')?;
    p.skip_whitespace();

    // Check optional tool description: " - Description text"
    let mut description = None;
    if p.peek() == Some('-') {
        p.bump();
        p.skip_whitespace();
        let rest_str = p.rest();
        let rest = rest_str.trim();
        if !rest.is_empty() {
            description = Some(collapse_whitespace(rest));
        }
    } else if !p.is_eof() {
        return Err(Error::MalformedCall(format!(
            "unexpected trailing characters after tool definition: '{}'",
            p.rest()
        )));
    }

    Ok(ToolSchema {
        name,
        description,
        params: fields,
    })
}

struct Parser<'a> {
    chars: Vec<char>,
    idx: usize,
    _src: &'a str,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            chars: src.chars().collect(),
            idx: 0,
            _src: src,
        }
    }

    fn is_eof(&self) -> bool {
        self.idx >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.idx).copied()
    }

    fn bump(&mut self) -> Option<char> {
        if self.idx < self.chars.len() {
            let c = self.chars[self.idx];
            self.idx += 1;
            Some(c)
        } else {
            None
        }
    }

    fn rest(&self) -> String {
        self.chars[self.idx..].iter().collect()
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c == ' ' || c == '\t' {
                self.bump();
            } else {
                break;
            }
        }
    }

    fn expect_char(&mut self, expected: char) -> Result<(), Error> {
        self.skip_whitespace();
        match self.bump() {
            Some(c) if c == expected => Ok(()),
            Some(c) => Err(Error::MalformedCall(format!(
                "expected '{expected}', found '{c}'"
            ))),
            None => Err(Error::MalformedCall(format!(
                "expected '{expected}', found end of input"
            ))),
        }
    }

    fn parse_ident(&mut self) -> Result<String, Error> {
        self.skip_whitespace();
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                s.push(c);
                self.bump();
            } else {
                break;
            }
        }
        if s.is_empty() {
            Err(Error::MalformedCall("expected identifier".to_string()))
        } else {
            Ok(s)
        }
    }

    fn parse_string_literal(&mut self) -> Result<String, Error> {
        self.skip_whitespace();
        self.expect_char('"')?;
        let mut s = String::new();
        let mut escape = false;
        while let Some(c) = self.bump() {
            if escape {
                match c {
                    '"' => s.push('"'),
                    '\\' => s.push('\\'),
                    '/' => s.push('/'),
                    'b' => s.push('\x08'),
                    'f' => s.push('\x0C'),
                    'n' => s.push('\n'),
                    'r' => s.push('\r'),
                    't' => s.push('\t'),
                    other => {
                        s.push('\\');
                        s.push(other);
                    }
                }
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                return Ok(s);
            } else {
                s.push(c);
            }
        }
        Err(Error::MalformedCall(
            "unterminated string literal".to_string(),
        ))
    }

    fn parse_field(&mut self) -> Result<Field, Error> {
        let name = self.parse_ident()?;
        let mut required = true;
        if self.peek() == Some('?') {
            self.bump();
            required = false;
        }

        self.expect_char(':')?;
        let ty = self.parse_type()?;

        // Check for field description string
        self.skip_whitespace();
        let description = if self.peek() == Some('"') {
            Some(self.parse_string_literal()?)
        } else {
            None
        };

        Ok(Field {
            name,
            required,
            ty,
            description,
        })
    }

    fn parse_type(&mut self) -> Result<Ty, Error> {
        self.skip_whitespace();
        match self.peek() {
            Some('[') => {
                self.bump();
                let inner = self.parse_type()?;
                self.expect_char(']')?;
                Ok(Ty::Array(Box::new(inner)))
            }
            Some('{') => {
                self.bump();
                let mut fields = Vec::new();
                self.skip_whitespace();
                if self.peek() != Some('}') {
                    loop {
                        p_skip(&mut self.idx, &self.chars);
                        let field = self.parse_field()?;
                        fields.push(field);
                        self.skip_whitespace();
                        if self.peek() == Some(',') {
                            self.bump();
                            self.skip_whitespace();
                        } else {
                            break;
                        }
                    }
                }
                self.expect_char('}')?;
                Ok(Ty::Object(fields))
            }
            _ => {
                let first_token = self.parse_ident()?;

                // Check if followed by pipe '|' (enum type)
                self.skip_whitespace();
                if self.peek() == Some('|') {
                    let mut variants = vec![first_token];
                    while self.peek() == Some('|') {
                        self.bump();
                        self.skip_whitespace();
                        let next_var = if self.peek() == Some('"') {
                            self.parse_string_literal()?
                        } else {
                            self.parse_ident()?
                        };
                        variants.push(next_var);
                        self.skip_whitespace();
                    }
                    return Ok(Ty::Enum(variants));
                }

                // Check standard types and formats
                match first_token.as_str() {
                    "str" => Ok(Ty::Str),
                    "int" => Ok(Ty::Int),
                    "num" => Ok(Ty::Num),
                    "bool" => Ok(Ty::Bool),
                    "null" => Ok(Ty::Null),
                    other => {
                        if let Some(fmt) = StrFormat::from_compact_name(other) {
                            Ok(Ty::Format(fmt))
                        } else {
                            // If an unquoted single-member enum or unknown type:
                            Err(Error::MalformedCall(format!("unknown type '{other}'")))
                        }
                    }
                }
            }
        }
    }
}

fn p_skip(idx: &mut usize, chars: &[char]) {
    while *idx < chars.len() && (chars[*idx] == ' ' || chars[*idx] == '\t') {
        *idx += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_tool() {
        let text = "get_weather(city:str, units?:celsius|fahrenheit) - Get the current weather.";
        let schemas = parse_compact_definitions(text).unwrap();
        assert_eq!(schemas.len(), 1);
        let s = &schemas[0];
        assert_eq!(s.name, "get_weather");
        assert_eq!(s.description.as_deref(), Some("Get the current weather."));
        assert_eq!(s.params.len(), 2);
        assert_eq!(s.params[0].name, "city");
        assert!(s.params[0].required);
        assert_eq!(s.params[0].ty, Ty::Str);
        assert_eq!(s.params[1].name, "units");
        assert!(!s.params[1].required);
        assert_eq!(
            s.params[1].ty,
            Ty::Enum(vec!["celsius".into(), "fahrenheit".into()])
        );
    }

    #[test]
    fn test_parse_complex_nested_and_formats() {
        let text = "create_event(title:str, start:datetime \"Start ISO 8601\", attendees?:[str], meta?:{id:uuid, active:bool})";
        let schemas = parse_compact_definitions(text).unwrap();
        assert_eq!(schemas.len(), 1);
        let s = &schemas[0];
        assert_eq!(s.name, "create_event");
        assert_eq!(s.params.len(), 4);
        assert_eq!(s.params[1].ty, Ty::Format(StrFormat::DateTime));
        assert_eq!(s.params[1].description.as_deref(), Some("Start ISO 8601"));
        assert_eq!(s.params[2].ty, Ty::Array(Box::new(Ty::Str)));

        if let Ty::Object(fields) = &s.params[3].ty {
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0].name, "id");
            assert_eq!(fields[0].ty, Ty::Format(StrFormat::Uuid));
            assert_eq!(fields[1].name, "active");
            assert_eq!(fields[1].ty, Ty::Bool);
        } else {
            panic!("expected Ty::Object");
        }
    }

    #[test]
    fn test_decode_tools_round_trip() {
        let compact = CompactTools {
            definitions: "search(query:str, limit?:int \"Max results\") - Search index".to_string(),
            instructions: "To call a tool, emit: <<call name {json args}>>".to_string(),
        };

        let tools = decode_tools(&compact).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].function.name, "search");
        assert_eq!(
            tools[0].function.description.as_deref(),
            Some("Search index")
        );
    }
}
