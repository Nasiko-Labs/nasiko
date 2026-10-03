# Task 10 — Stream decoder

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

`stream.rs`:

```rust
pub struct StreamDecoder { buf: String, emitted_upto: usize }

impl StreamDecoder {
    pub fn new() -> Self;
    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>, CompactError>;
    pub fn finish(&mut self) -> Result<Vec<ToolCall>, CompactError>;
}
```

`push` appends `chunk` and scans `buf` from `emitted_upto`. Complete calls are returned
and `emitted_upto` moves past them. An incomplete tail stays in `buf`. `finish` errors
with `Malformed` when the tail still contains `<<call` without a closer, and returns
empty when the tail is prose or empty.

Hold the `ToolDef` slice on the decoder (`StreamDecoder::new(tools)`), because `push`
must validate. Do not emit a call before `check` succeeds.

### Tests

```rust
#[test]
fn marker_split_across_chunks_emits_one_call_at_the_end() {
    let mut dec = StreamDecoder::new(&[calendar()]);
    assert!(dec.push("<<ca").unwrap().is_empty());
    assert!(dec.push(r#"ll create_calendar_event {"title":"Ret"#).unwrap().is_empty());
    assert!(dec.push(r#"ro","start":"2026-10-04T10:00:00+05:30"}>"#).unwrap().is_empty());
    let calls = dec.push(">").unwrap();
    assert_eq!(calls.len(), 1);
    let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(args["title"], "Retro");
}

#[test]
fn one_chunk_matches_decode_calls() {
    let mut dec = StreamDecoder::new(&[calendar()]);
    let streamed = dec.push(design_review()).unwrap();
    let direct = decode_calls(design_review(), &[calendar()]).unwrap();
    assert_eq!(streamed, direct);
}

#[test]
fn bad_enum_across_chunks_is_an_error_with_no_call() {
    let mut dec = StreamDecoder::new(&[calendar()]);
    dec.push(r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30","visibility":"sec"#).unwrap();
    let err = dec.push(r#"ret"}>>"#).unwrap_err();
    assert!(matches!(err, CompactError::InvalidArguments { reason: ArgumentFault::BadEnum { .. }, .. }));
}
```
