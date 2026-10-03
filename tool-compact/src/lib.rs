use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A minimal ToolDef compatible shape (keeps JSON Schema as Value)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    /// JSON Schema for parameters (may be omitted)
    pub parameters: Option<Value>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    /// arguments as JSON string (OpenAI shape)
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompactTools {
    /// compact representation as a single string (implementation detail)
    pub rendered: String,
}

impl CompactTools {
    pub fn new(rendered: String) -> Self {
        Self { rendered }
    }
}

/// Encode full tool definitions into a compact textual representation.
/// This implementation is intentionally simple: it renders a short-line
/// declaration per tool and a single call-format instruction.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut parts = Vec::new();
    for t in tools {
        // simple compact: name(arg1:type,arg2?:type) - desc
        let sig = format!("{}()", t.name);
        let desc = t.description.clone().unwrap_or_default();
        parts.push(format!("{} - {}", sig, desc));
    }
    parts.push("To call a tool, emit: <<call name {json args}>>".to_string());
    Ok(CompactTools::new(parts.join("\n")))
}

/// Decode a plain text output into ToolCall(s). Finds occurrences of
/// <<call NAME JSON>> and returns parsed ToolCall entries.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<<call ") {
        rest = &rest[start + "<<call ".len()..];
        // find the closing >>
        if let Some(end_idx) = rest.find(">>") {
            let inner = &rest[..end_idx];
            // split at first whitespace
            let mut parts = inner.splitn(2, ' ');
            let name = parts
                .next()
                .ok_or_else(|| anyhow!("malformed call: empty name"))?
                .trim()
                .to_string();
            let json_part = parts
                .next()
                .ok_or_else(|| anyhow!("malformed call: missing json"))?
                .trim();
            // validate tool exists
            if !tools.iter().any(|t| t.name == name) {
                return Err(anyhow!("unknown_tool: {}", name));
            }
            // ensure json parses
            let _v: Value = serde_json::from_str(json_part)
                .map_err(|e| anyhow!("invalid_arguments: {}", e))?;
            out.push(ToolCall { name, arguments: json_part.to_string() });
            rest = &rest[end_idx + 2..];
        } else {
            // no closing marker; stop
            break;
        }
    }
    Ok(out)
}

/// An incremental stream decoder that accepts chunks and yields any completed calls.
pub struct StreamDecoder {
    buffer: String,
    tools: Vec<ToolDef>,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Self {
        Self { buffer: String::new(), tools: tools.to_vec() }
    }

    /// Push a chunk of text; returns any newly-decoded calls.
    pub fn push_chunk(&mut self, chunk: &str) -> Result<Vec<ToolCall>> {
        self.buffer.push_str(chunk);
        // attempt to decode all complete markers
        let decoded = decode_calls(&self.buffer, &self.tools)?;
        // remove fully consumed parts by searching for last closing >>
        if let Some(pos) = self.buffer.rfind(">>") {
            // keep remaining after last >>
            self.buffer = self.buffer[pos+2..].to_string();
        }
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_simple() {
        let tools = vec![ToolDef { name: "create_calendar_event".into(), parameters: None, description: Some("Create event".into()) }];
        let ct = encode_tools(&tools).unwrap();
        assert!(ct.rendered.contains("create_calendar_event"));
        let out = decode_calls("hello <<call create_calendar_event {\"title\":\"A\"}>> world", &tools).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "create_calendar_event");
    }

    #[test]
    fn stream_decoder_handles_split_marker() {
        let tools = vec![ToolDef { name: "create_calendar_event".into(), parameters: None, description: None }];
        let mut dec = StreamDecoder::new(&tools);
        let parts = ["<<ca", "ll create_calendar_event {\"title\":\"Ret", "ro\"}> ">>"];
        // push sequentially
        let mut total = Vec::new();
        for p in parts.iter() {
            let d = dec.push_chunk(p).unwrap();
            total.extend(d);
        }
        // should have one decoded call
        assert_eq!(total.len(), 1);
        assert_eq!(total[0].name, "create_calendar_event");
    }
}
