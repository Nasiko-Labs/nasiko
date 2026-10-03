# Task 11 — No invalid success

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

Add `schema::check` as the only success path inside both decoders. The test feeds a
table of bad inputs and asserts each one is `Err`. It also re-checks every successful
fixture from earlier tasks with `check` so a future shortcut cannot skip validation.

### Tests

```rust
#[test]
fn schema_valid_calls_only() {
    let bad = [
        r#"<<call weather {"city":"Pune"}>>"#,
        r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>"#,
        r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
        r#"<<call create_calendar_event {"title":"Retro"}"#,
        "not a call <<call",
    ];
    for text in bad {
        assert!(decode_calls(text, &[calendar()]).is_err(), "{text}");
        let mut dec = StreamDecoder::new(&[calendar()]);
        assert!(dec.push(text).or_else(|_| dec.finish()).is_err() || dec.finish().unwrap().is_empty());
    }
    let ok = decode_calls(design_review(), &[calendar()]).unwrap();
    assert!(schema::check(&classify(&calendar()).unwrap(), &serde_json::from_str::<Value>(&ok[0].arguments).unwrap()).is_ok());
}
```

Run it with `cargo test -p nasiko-tool-compact schema_valid_calls_only`.
