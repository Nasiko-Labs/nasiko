//! Incremental decoding. Text streams through with a bounded hold-back: only a possible
//! start of [`MARKER`] (at most 5 bytes) or an unfinished call is buffered. Any chunking of
//! an input gives the same calls (or error) and the same text as decoding it whole (I6).

use crate::calls::{MARKER, Scan, finish_call, scan_call};
use crate::{Error, ToolCall, ToolDef};

/// Longest call kept in the buffer before giving up; bounds memory on a runaway call.
pub const MAX_CALL_BYTES: usize = 1 << 20;

/// What the decoder hands back as input arrives.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// Prose safe to show the user now.
    Text(String),
    /// A complete, validated call.
    Call(ToolCall),
}

/// Feed chunks with [`push`](Self::push), then call [`finish`](Self::finish). After an
/// error the decoder stays failed; the caller must treat the whole response as failed.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buf: String,
    failed: Option<Error>,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            tools: tools.to_vec(),
            buf: String::new(),
            failed: None,
        }
    }

    pub fn push(&mut self, chunk: &str) -> Result<Vec<StreamEvent>, Error> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        self.buf.push_str(chunk);
        let out = self.drain();
        if let Err(e) = &out {
            self.failed = Some(e.clone());
        }
        out
    }

    /// End of input: an unfinished call is an error; a dangling marker prefix is text.
    pub fn finish(mut self) -> Result<Vec<StreamEvent>, Error> {
        if let Some(e) = self.failed {
            return Err(e);
        }
        if self.buf.starts_with(MARKER) {
            return Err(Error::Malformed("unterminated call".into()));
        }
        let rest = std::mem::take(&mut self.buf);
        Ok(if rest.is_empty() {
            Vec::new()
        } else {
            vec![StreamEvent::Text(rest)]
        })
    }

    fn drain(&mut self) -> Result<Vec<StreamEvent>, Error> {
        let mut events = Vec::new();
        loop {
            let Some(at) = self.buf.find(MARKER) else {
                // Hold back the longest suffix that could still become a marker.
                let keep = (1..MARKER.len())
                    .rev()
                    .find(|&n| {
                        self.buf
                            .get(self.buf.len().checked_sub(n).unwrap_or(usize::MAX)..)
                            .is_some_and(|s| MARKER.starts_with(s))
                    })
                    .unwrap_or(0);
                let text: String = self.buf.drain(..self.buf.len() - keep).collect();
                push_text(&mut events, text);
                return Ok(events);
            };
            let text: String = self.buf.drain(..at).collect();
            push_text(&mut events, text);
            let used = match scan_call(&self.buf)? {
                Scan::Call { name, json, used } => {
                    events.push(StreamEvent::Call(finish_call(name, json, &self.tools)?));
                    used
                }
                Scan::Incomplete if self.buf.len() > MAX_CALL_BYTES => {
                    return Err(Error::Malformed("call too long".into()));
                }
                Scan::Incomplete => return Ok(events),
            };
            self.buf.drain(..used);
        }
    }
}

fn push_text(events: &mut Vec<StreamEvent>, text: String) {
    if !text.is_empty() {
        events.push(StreamEvent::Text(text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_calls;
    use serde_json::json;

    fn tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "create_calendar_event".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string", "format": "date-time"},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        }]
    }

    /// (calls or error label, all text) from feeding `chunks`.
    fn run(chunks: &[&str]) -> (Result<Vec<ToolCall>, &'static str>, String) {
        let mut d = StreamDecoder::new(&tools());
        let mut events = Vec::new();
        for c in chunks {
            match d.push(c) {
                Ok(e) => events.extend(e),
                Err(e) => return (Err(e.as_label()), String::new()),
            }
        }
        match d.finish() {
            Ok(e) => events.extend(e),
            Err(e) => return (Err(e.as_label()), String::new()),
        }
        let mut calls = Vec::new();
        let mut text = String::new();
        for e in events {
            match e {
                StreamEvent::Text(t) => text.push_str(&t),
                StreamEvent::Call(c) => calls.push(c),
            }
        }
        (Ok(calls), text)
    }

    const INPUTS: &[&str] = &[
        "Plain answer, no call. a << b <",
        "Booking.\n<<call create_calendar_event {\"title\":\"Retro >> }\\\"\",\"start\":\"2026-10-04T10:00:00+05:30\"}>> done <<",
        "<<call create_calendar_event {\"title\":\"a\",\"start\":\"2026-10-04T10:00:00Z\"}>><<call create_calendar_event {\"title\":\"b\",\"start\":\"2026-10-05T10:00:00Z\",\"visibility\":\"private\"}>>",
        "x <<call create_calendar_event {\"title\":\"a\",\"start\":\"2026-10-04T10:00:00Z\",\"visibility\":\"secret\"}>>",
        "x <<call delete_everything {}>>",
        "x <<call create_calendar_event {\"title\":\"a\"",
        "ünïcode <<<call create_calendar_event {\"title\":\"é\",\"start\":\"2026-10-04T10:00Z\"}  >> ✓",
        "<<cal",
    ];

    #[test]
    fn markers_split_across_chunks() {
        let (calls, text) = run(&[
            "Hi <",
            "<ca",
            "ll create_calendar_event {\"ti",
            "tle\":\"a\",\"start\":\"2026-10-04T10:00Z\"}",
            ">",
            "> bye",
        ]);
        assert_eq!(calls.unwrap().len(), 1);
        assert_eq!(text, "Hi  bye");
    }

    #[test]
    fn hold_back_is_bounded() {
        let mut d = StreamDecoder::new(&tools());
        let e = d.push("long text then <<ca").unwrap();
        assert_eq!(e, vec![StreamEvent::Text("long text then ".into())]);
        let mut d = StreamDecoder::new(&tools());
        d.push("<<call create_calendar_event {\"title\":\"")
            .unwrap();
        let big = "x".repeat(MAX_CALL_BYTES);
        assert_eq!(d.push(&big).unwrap_err().as_label(), "malformed");
        assert!(d.push("\"}>>").is_err(), "stays failed");
    }

    /// I6: every chunking gives the same result as whole-input decoding.
    #[test]
    fn i6_any_chunking_equals_whole_decode() {
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut rand = move |n: usize| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as usize % n.max(1)
        };
        for input in INPUTS {
            let whole = decode_calls(input, &tools()).map_err(|e| e.as_label());
            let reference = run(&[input]);
            assert_eq!(reference.0, whole, "{input}");
            let bounds: Vec<usize> = input
                .char_indices()
                .map(|(i, _)| i)
                .chain([input.len()])
                .collect();
            // Every single split point, then random multi-way splits.
            let mut splits: Vec<Vec<usize>> = bounds.iter().map(|&b| vec![b]).collect();
            for _ in 0..300 {
                let mut cut: Vec<usize> =
                    (0..rand(12)).map(|_| bounds[rand(bounds.len())]).collect();
                cut.sort_unstable();
                splits.push(cut);
            }
            splits.push(bounds.clone()); // one char per chunk
            for cut in splits {
                let mut chunks = Vec::new();
                let mut last = 0;
                for &c in cut.iter().chain([&input.len()]) {
                    chunks.push(input.get(last..c).unwrap());
                    last = c;
                }
                assert_eq!(run(&chunks), reference, "{input} split at {cut:?}");
            }
        }
    }
}
