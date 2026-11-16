//! Pure parser for OpenAI-style streamed chat completions (`stream: true`).
//!
//! Feed raw body bytes with [`StreamAccumulator::push_bytes`]; it returns text
//! deltas to forward to the user and accumulates tool calls by index. When the
//! stream ends, [`StreamAccumulator::finish`] gives the assistant message in the
//! same shape a non-streamed response has (`content` + `tool_calls`).

use serde_json::{Value, json};

#[derive(Debug, Default, Clone)]
struct ToolCallBuf {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, Default)]
pub struct StreamAccumulator {
    line_buf: Vec<u8>,
    content: String,
    tool_calls: Vec<ToolCallBuf>,
    finish_reason: Option<String>,
    error: Option<String>,
    done: bool,
}

impl StreamAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consumes a body chunk; returns the text deltas it completed.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Vec<String> {
        self.line_buf.extend_from_slice(bytes);
        let mut deltas: Vec<String> = Vec::new();
        while let Some(pos) = self.line_buf.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = self.line_buf.drain(..=pos).collect();
            let line_raw = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]);
            let line: &str = line_raw.trim_end_matches('\r');
            if let Some(d) = self.push_line(line) {
                deltas.push(d);
            }
        }
        deltas
    }

    fn push_line(&mut self, line: &str) -> Option<String> {
        let data: &str = line.strip_prefix("data:")?.trim();
        if data == "[DONE]" {
            self.done = true;
            return None;
        }
        let chunk: Value = serde_json::from_str(data).ok()?;
        if let Some(err) = chunk.get("error").filter(|e| !e.is_null()) {
            let msg = err
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| err.to_string());
            self.error = Some(msg);
            return None;
        }
        if let Some(reason) = chunk["choices"][0]["finish_reason"].as_str() {
            self.finish_reason = Some(reason.to_owned());
        }
        let delta: &Value = &chunk["choices"][0]["delta"];
        if let Some(calls) = delta["tool_calls"].as_array() {
            for c in calls {
                let idx: usize = c["index"].as_u64().unwrap_or(0) as usize;
                if self.tool_calls.len() <= idx {
                    self.tool_calls.resize(idx + 1, ToolCallBuf::default());
                }
                let buf: &mut ToolCallBuf = &mut self.tool_calls[idx];
                if let Some(id) = c["id"].as_str() {
                    buf.id.push_str(id);
                }
                if let Some(n) = c["function"]["name"].as_str() {
                    buf.name.push_str(n);
                }
                if let Some(a) = c["function"]["arguments"].as_str() {
                    buf.arguments.push_str(a);
                }
            }
        }
        let text: &str = delta["content"].as_str().unwrap_or("");
        if text.is_empty() {
            return None;
        }
        self.content.push_str(text);
        Some(text.to_owned())
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn finish_reason(&self) -> Option<&str> {
        self.finish_reason.as_deref()
    }

    /// The assistant message, shaped like a non-streamed `choices[0].message`.
    pub fn finish(mut self) -> Value {
        let rest_bytes: Vec<u8> = std::mem::take(&mut self.line_buf);
        let rest: String = String::from_utf8_lossy(&rest_bytes).into_owned();
        if !rest.trim().is_empty() {
            self.push_line(rest.trim());
        }
        let mut msg: Value = json!({"role": "assistant", "content": self.content});
        if !self.tool_calls.is_empty() {
            let calls: Vec<Value> = self
                .tool_calls
                .iter()
                .map(|c| {
                    json!({"id": c.id, "type": "function",
                    "function": {"name": c.name, "arguments": c.arguments}})
                })
                .collect();
            msg["tool_calls"] = json!(calls);
        }
        msg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_deltas_stream_across_split_chunks() {
        let mut acc = StreamAccumulator::new();
        let a: Vec<String> = acc.push_bytes(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\ndata: {\"choi",
        );
        assert_eq!(a, vec!["Hel"]);
        let b: Vec<String> =
            acc.push_bytes(b"ces\":[{\"delta\":{\"content\":\"lo\"}}]}\n\ndata: [DONE]\n");
        assert_eq!(b, vec!["lo"]);
        assert!(acc.is_done());
        let msg: Value = acc.finish();
        assert_eq!(msg["content"], "Hello");
        assert!(msg.get("tool_calls").is_none());
    }

    #[test]
    fn tool_calls_accumulate_by_index() {
        let mut acc = StreamAccumulator::new();
        acc.push_bytes(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"GMAIL_FETCH\",\"arguments\":\"{\\\"q\\\"\"}}]}}]}\n");
        acc.push_bytes(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":1}\"}}]}}]}\n");
        let msg: Value = acc.finish();
        assert_eq!(msg["tool_calls"][0]["id"], "c1");
        assert_eq!(msg["tool_calls"][0]["function"]["name"], "GMAIL_FETCH");
        assert_eq!(msg["tool_calls"][0]["function"]["arguments"], "{\"q\":1}");
    }

    #[test]
    fn multibyte_utf8_split_across_chunks() {
        let mut acc = StreamAccumulator::new();
        // "😀" is 4 UTF-8 bytes: F0 9F 98 80
        let mut chunk1 = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi \xF0\x9F".to_vec();
        let mut chunk2 = b"\x98\x80\"}}]}\n\ndata: [DONE]\n".to_vec();
        assert!(acc.push_bytes(&chunk1).is_empty());
        let out: Vec<String> = acc.push_bytes(&chunk2);
        assert_eq!(out, vec!["hi 😀"]);
        assert!(acc.is_done());
        let _ = (&mut chunk1, &mut chunk2);
    }
}
