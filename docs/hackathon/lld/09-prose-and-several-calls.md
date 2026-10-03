# Task 9 — Prose and several calls

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

The scanner finds every complete marker and ignores the gaps. After the last marker,
trailing prose is ignored. Zero markers is `Ok(vec![])`. A later marker is not parsed
when an earlier one already failed validation: return that error. Order of `Ok` calls
is source order.

### Tests

```rust
#[test]
fn prose_around_a_call_is_ignored() {
    let text = format!("Sure.\n{design_review}\nDone.", design_review = design_review());
    let calls = decode_calls(&text, &[calendar()]).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
}

#[test]
fn two_calls_come_back_in_source_order() {
    let text = r#"<<call send_email {"to":["sam@example.com"]}>> then <<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}>>"#;
    let email = ToolDef {
        name: "send_email".into(),
        description: None,
        parameters: Some(json!({"type":"object","properties":{"to":{"type":"array","items":{"type":"string"}}},"required":["to"]})),
    };
    let calls = decode_calls(text, &[calendar(), email]).unwrap();
    assert_eq!(calls[0].name, "send_email");
    assert_eq!(calls[1].name, "create_calendar_event");
}

#[test]
fn plain_answer_has_no_calls_and_is_not_an_error() {
    let calls = decode_calls("What's the weather?", &[calendar()]).unwrap();
    assert!(calls.is_empty());
}
```
