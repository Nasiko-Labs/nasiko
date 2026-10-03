# Task 18 — Bypass

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

Inside `apply`, before encode:

- any tool `classify` returns `UnsupportedSchema` → `Skip::UnsupportedSchema`
- `tool_choice` is present and is not JSON `"auto"` → `Skip::ForcedToolChoice`
  (`"none"`, `{"type":"function","function":{"name":"..."}}`, and any other value)
- provider is not `openai`, or `is_streaming()` → `Skip::UnsupportedPath` (already task 17)

On every skip, `req` is unchanged, `applied` is false, and `skip` is set.

### Tests

```rust
#[test]
fn unsupported_schema_keeps_native_tools() {
    let mut req = sample_chat_request();
    req.tools.as_mut().unwrap()[0].function.parameters = Some(json!({"$ref": "#/$defs/Id"}));
    let before = serde_json::to_vec(&req).unwrap();
    let outcome = compact_tools::apply(&mut req, true, "openai");
    assert!(matches!(outcome.skip, Some(Skip::UnsupportedSchema)));
    assert_eq!(serde_json::to_vec(&req).unwrap(), before);
}

#[test]
fn forced_tool_choice_keeps_native_tools() {
    let mut req = sample_chat_request();
    req.tool_choice = Some(json!({"type": "function", "function": {"name": "create_calendar_event"}}));
    let before = serde_json::to_vec(&req).unwrap();
    let outcome = compact_tools::apply(&mut req, true, "openai");
    assert!(matches!(outcome.skip, Some(Skip::ForcedToolChoice)));
    assert_eq!(serde_json::to_vec(&req).unwrap(), before);
}

#[test]
fn gemini_streaming_keeps_native_tools() {
    let mut req = sample_chat_request();
    req.stream = Some(true);
    let before = serde_json::to_vec(&req).unwrap();
    let outcome = compact_tools::apply(&mut req, true, "gemini");
    assert!(matches!(outcome.skip, Some(Skip::UnsupportedPath)));
    assert_eq!(serde_json::to_vec(&req).unwrap(), before);
}
```
