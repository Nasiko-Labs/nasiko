# Task 8 — Fail closed

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

`schema::check(shape, value) -> Result<(), ArgumentFault>`:

- missing required field → `MissingField`
- JSON type does not match the shape → `WrongType`
- string not in the enum → `BadEnum`
- extra fields the schema does not list → `WrongType` on that field name
- JSON parse failure, missing closer, or a non-object argument → `InvalidArguments` with
  `ArgumentFault::Malformed` and the tool name when one was read, otherwise the name is empty

`decode_calls` returns `Err` and never an `Ok` that also contains a bad call. There is
no partial-success vec.

### Tests

```rust
#[test]
fn unknown_tool_is_unknown_tool() {
    let text = r#"<<call weather {"city":"Pune"}>>"#;
    let err = decode_calls(text, &[calendar()]).unwrap_err();
    assert!(matches!(err, CompactError::UnknownTool { name } if name == "weather"));
}

#[test]
fn missing_title_is_invalid_arguments() {
    let text = r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>"#;
    let err = decode_calls(text, &[calendar()]).unwrap_err();
    assert!(matches!(err, CompactError::InvalidArguments { reason: ArgumentFault::MissingField(f), .. } if f == "title"));
}

#[test]
fn string_duration_is_wrong_type() {
    let text = r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-05T15:00:00+05:30","duration_min":"30"}>>"#;
    let err = decode_calls(text, &[calendar()]).unwrap_err();
    assert!(matches!(err, CompactError::InvalidArguments { reason: ArgumentFault::WrongType { field }, .. } if field == "duration_min"));
}

#[test]
fn secret_visibility_is_a_bad_enum() {
    let text = r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
    let err = decode_calls(text, &[calendar()]).unwrap_err();
    assert!(matches!(err, CompactError::InvalidArguments { reason: ArgumentFault::BadEnum { field }, .. } if field == "visibility"));
}

#[test]
fn truncated_marker_is_malformed() {
    let text = r#"<<call create_calendar_event {"title":"Retro"}"#;
    let err = decode_calls(text, &[calendar()]).unwrap_err();
    assert!(matches!(err, CompactError::InvalidArguments { reason: ArgumentFault::Malformed, .. }));
}
```
