//! Strict JSON tokenizer and duplicate-key detection.

use crate::error::Error;

const MAX_JSON_DEPTH: usize = 64;

/// Scans a JSON string and verifies that no object contains duplicate keys at any depth.
///
/// Returns `Err(Error::InvalidArguments)` if a duplicate key or malformed syntax is detected,
/// or `Err(Error::LimitExceeded)` if nesting depth exceeds safety limits.
pub fn check_duplicate_keys(s: &str) -> Result<(), Error> {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    // Stack of (is_object, keys_at_level)
    let mut scopes: Vec<(bool, Vec<String>)> = Vec::new();

    while i < len {
        match bytes[i] {
            b'{' => {
                if scopes.len() >= MAX_JSON_DEPTH {
                    return Err(Error::LimitExceeded("nesting depth exceeded 64"));
                }
                scopes.push((true, Vec::new()));
                i += 1;
            }
            b'}' => {
                scopes.pop();
                i += 1;
            }
            b'[' => {
                if scopes.len() >= MAX_JSON_DEPTH {
                    return Err(Error::LimitExceeded("nesting depth exceeded 64"));
                }
                scopes.push((false, Vec::new()));
                i += 1;
            }
            b']' => {
                scopes.pop();
                i += 1;
            }
            b'"' => {
                let start = i + 1;
                i += 1;
                let mut escape = false;
                while i < len {
                    if escape {
                        escape = false;
                    } else if bytes[i] == b'\\' {
                        escape = true;
                    } else if bytes[i] == b'"' {
                        break;
                    }
                    i += 1;
                }

                if i >= len {
                    return Err(Error::InvalidArguments(
                        "unterminated string literal in JSON arguments".to_string(),
                    ));
                }

                let end = i;
                i += 1; // skip closing quote

                // If currently inside an Object and followed by ':', this string was an object key!
                if let Some((true, keys)) = scopes.last_mut() {
                    let mut lookahead = i;
                    while lookahead < len && bytes[lookahead].is_ascii_whitespace() {
                        lookahead += 1;
                    }
                    if lookahead < len && bytes[lookahead] == b':' {
                        let key_str = std::str::from_utf8(&bytes[start..end]).map_err(|_| {
                            Error::InvalidArguments("invalid UTF-8 in JSON key".to_string())
                        })?;

                        // Unescape key string to handle e.g. "a\u0020b" or "a\"b"
                        let unescaped_key = unescape_json_string(key_str)?;

                        if keys.iter().any(|k| k == &unescaped_key) {
                            return Err(Error::InvalidArguments(format!(
                                "duplicate key '{unescaped_key}' in JSON arguments"
                            )));
                        }
                        keys.push(unescaped_key);
                    }
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    Ok(())
}

fn unescape_json_string(s: &str) -> Result<String, Error> {
    if !s.contains('\\') {
        return Ok(s.to_string());
    }

    let mut res = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => res.push('"'),
                Some('\\') => res.push('\\'),
                Some('/') => res.push('/'),
                Some('b') => res.push('\x08'),
                Some('f') => res.push('\x0c'),
                Some('n') => res.push('\n'),
                Some('r') => res.push('\r'),
                Some('t') => res.push('\t'),
                Some('u') => {
                    let mut hex = String::with_capacity(4);
                    for _ in 0..4 {
                        match chars.next() {
                            Some(h) if h.is_ascii_hexdigit() => hex.push(h),
                            _ => {
                                return Err(Error::InvalidArguments(
                                    "invalid \\u escape sequence".to_string(),
                                ));
                            }
                        }
                    }
                    let code = u32::from_str_radix(&hex, 16).map_err(|_| {
                        Error::InvalidArguments("invalid hex in \\u escape".to_string())
                    })?;
                    let ch = char::from_u32(code).ok_or_else(|| {
                        Error::InvalidArguments("invalid Unicode code point".to_string())
                    })?;
                    res.push(ch);
                }
                _ => {
                    return Err(Error::InvalidArguments(
                        "invalid escape sequence in JSON string".to_string(),
                    ));
                }
            }
        } else {
            res.push(c);
        }
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_json_keys() {
        assert!(check_duplicate_keys(r#"{"a": 1, "b": 2}"#).is_ok());
        assert!(check_duplicate_keys(r#"{"nested": {"x": 10, "y": 20}}"#).is_ok());
        assert!(check_duplicate_keys(r#"[{"a": 1}, {"a": 2}]"#).is_ok());
    }

    #[test]
    fn test_duplicate_key_root() {
        let err = check_duplicate_keys(r#"{"a": 1, "a": 2}"#).unwrap_err();
        assert!(matches!(err, Error::InvalidArguments(_)));
        assert!(err.to_string().contains("duplicate key 'a'"));
    }

    #[test]
    fn test_duplicate_key_nested() {
        let err = check_duplicate_keys(r#"{"parent": {"child": 1, "child": 2}}"#).unwrap_err();
        assert!(matches!(err, Error::InvalidArguments(_)));
        assert!(err.to_string().contains("duplicate key 'child'"));
    }

    #[test]
    fn test_duplicate_key_in_array_object() {
        let err = check_duplicate_keys(r#"[{"k": 1, "k": 2}]"#).unwrap_err();
        assert!(matches!(err, Error::InvalidArguments(_)));
        assert!(err.to_string().contains("duplicate key 'k'"));
    }

    #[test]
    fn test_colons_and_braces_in_strings() {
        assert!(check_duplicate_keys(r#"{"msg": "foo:bar", "code": "{hello}"}"#).is_ok());
    }

    #[test]
    fn test_escaped_quotes_and_slashes() {
        assert!(check_duplicate_keys(r#"{"a\"b": 1, "path": "C:\\temp"}"#).is_ok());
        let err = check_duplicate_keys(r#"{"a\"b": 1, "a\"b": 2}"#).unwrap_err();
        assert!(err.to_string().contains("duplicate key 'a\"b'"));
    }

    #[test]
    fn test_unterminated_string() {
        assert!(check_duplicate_keys(r#"{"a: 1}"#).is_err());
    }

    #[test]
    fn test_excessive_depth() {
        let deep = "{".repeat(70) + &"}".repeat(70);
        let err = check_duplicate_keys(&deep).unwrap_err();
        assert!(matches!(err, Error::LimitExceeded(_)));
    }
}
