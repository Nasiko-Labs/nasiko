# nasiko-tool-compact

Compact tool schemas for LLM tool calling: a one-line-per-tool definition format
that replaces native JSON Schema blobs in the prompt, plus a fail-closed decoder
that turns the model's `<<call name {json}>>` markers back into standard tool
calls validated against the original schema.

- Offline, deterministic, no I/O (pure parsing/validation).
- Round-trippable: `decode_tools(encode_tools(t))` preserves schema semantics
  (required vs optional, types, enums, nested objects/arrays).
- Streaming-safe: `StreamDecoder` buffers split markers and never terminates a
  call on `>>` inside a JSON string literal.
- Fail-closed: unknown tool, missing required field, or schema violation is an
  error, never a guessed call.

Schema features outside the supported subset (`anyOf`/`oneOf`/`allOf`, `$ref`,
non-string enums, free-form objects) are rejected by `encode_tools`; callers
must bypass compaction for those requests.

The router wires this opt-in (`TOKEN_COMPACT_TOOLS`, default off): non-streaming
requests have their native `tools` replaced with an injected system message, and
compact call markers in the response text are decoded back into standard
`tool_calls`. Streaming and `tool_choice` requests bypass compaction.

See `llm-router/src/compact_tools.rs` for the seam and
`llm-router/examples/compact_tools_eval.rs` for the eval harness.
