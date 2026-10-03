# Task 7 — Closer inside a string

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

Extend the scanner with a state: `Outside`, `InString`, `Escaped`. `"` toggles
`InString`. `\` inside a string enters `Escaped` for one character. `>>` ends the call
only in `Outside`. Track the index by `char` boundaries, never by bytes.

### Tests

```rust
#[test]
fn greater_than_inside_a_string_stays_in_the_title() {
    let text = r#"<<call create_calendar_event {"title":"meet >> review","start":"2026-10-05T15:00:00+05:30"}>> trailing"#;
    let calls = decode_calls(text, &[calendar()]).unwrap();
    let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(args["title"], "meet >> review");
    assert!(!calls[0].arguments.contains("trailing"));
}

#[test]
fn escaped_quote_does_not_end_the_json_string() {
    let text = r#"<<call create_calendar_event {"title":"say \"hi\"","start":"2026-10-05T15:00:00+05:30"}>>"#;
    let calls = decode_calls(text, &[calendar()]).unwrap();
    let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(args["title"], "say \"hi\"");
}
```
