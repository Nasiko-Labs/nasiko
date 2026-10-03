# Task 6 — Decode one call

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

`grammar.rs` scans from the first `<<call `, reads a tool name of `[A-Za-z0-9_]+`,
requires one space, then a JSON object, then `>>` outside strings. `decode.rs`
`decode_calls` uses that scanner, checks the name against `tools`, then `schema::check`.
On success, `arguments` is the JSON object text with whitespace unchanged inside the
object. Object key order is not significant: parse both sides as `serde_json::Value`.

A call is not validated until task 8. This task only accepts a call that already
matches the schema, so the test stays valid after validation lands.

### Tests

```rust
#[test]
fn design_review_call_decodes_to_one_tool_call() {
    let calls = decode_calls(design_review(), &[calendar()]).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
    let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(args["title"], "Design review");
    assert_eq!(args["start"], "2026-10-05T15:00:00+05:30");
    assert_eq!(args["attendees"], json!(["riya@example.com"]));
}

#[test]
fn argument_key_order_does_not_matter() {
    let text = r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","title":"Design review"}>>"#;
    let calls = decode_calls(text, &[calendar()]).unwrap();
    let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(args["title"], "Design review");
    assert_eq!(args["start"], "2026-10-05T15:00:00+05:30");
}
```
