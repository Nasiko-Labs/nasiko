# Task 17 — OpenAI non-streaming path

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

When `enabled && provider == "openai" && !req.is_streaming()` and every tool classifies
and `tool_choice` is absent or `"auto"`:

1. Convert router `ToolDef` to the crate `ToolDef` (name, description, parameters).
2. `encode_tools`. On `Ok`, append a system `Message` whose content is `CompactTools.text`,
   set `req.tools = None`, return `applied: true`.
3. Keep the original tools in a side value on `Outcome` (`originals: Vec<ToolDef>`) for
   the response path. Do not put them back on the request.

Response path, OpenAI non-streaming only: if this request was compacted, take the
assistant message content, `decode_calls` with the originals, and replace that content's
tool calls with router `ToolCall`s. Assign `id` as `call_{index}` starting at 1.
`arguments` stays the JSON string. On `Err`, leave the provider error path; do not invent
a tool call. The seam function that does this is `decode_response(text, originals) -> Result<Vec<ir::ToolCall>, CompactError>`.

Streaming, Anthropic, and Gemini: `apply` returns `Skip::UnsupportedPath` and does not
mutate.

### Tests

```rust
#[test]
fn enabled_openai_request_drops_native_tools_and_adds_the_compact_text() {
    let mut req = sample_chat_request();
    let outcome = compact_tools::apply(&mut req, true, "openai");
    assert!(outcome.applied);
    assert!(req.tools.is_none());
    let blob = serde_json::to_string(&req).unwrap();
    assert!(blob.contains("<<call name {json args}>>"));
    assert!(blob.contains("create_calendar_event("));
}

#[test]
fn decoded_reply_is_an_openai_tool_call() {
    let calls = compact_tools::decode_response(design_review(), &[router_calendar()]).unwrap();
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].function.name, "create_calendar_event");
    let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["title"], "Design review");
}

#[test]
fn streaming_and_other_providers_are_not_rewritten() {
    let mut streaming = sample_chat_request();
    streaming.stream = Some(true);
    let before = serde_json::to_vec(&streaming).unwrap();
    assert!(matches!(compact_tools::apply(&mut streaming, true, "openai").skip, Some(Skip::UnsupportedPath)));
    assert_eq!(serde_json::to_vec(&streaming).unwrap(), before);

    let mut anthropic = sample_chat_request();
    let before = serde_json::to_vec(&anthropic).unwrap();
    assert!(matches!(compact_tools::apply(&mut anthropic, true, "anthropic").skip, Some(Skip::UnsupportedPath)));
    assert_eq!(serde_json::to_vec(&anthropic).unwrap(), before);
}
```
