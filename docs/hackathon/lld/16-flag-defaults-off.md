# Task 16 — Flag defaults off

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

`GatewayConfig` gains `compact_tools_enabled: bool`, filled by
`env_flag("COMPACT_TOOLS_ENABLED", false)` inside `from_env`. Default struct literal
used in tests sets `false`.

`llm-router/src/compact_tools.rs`:

```rust
pub enum Skip { Disabled, UnsupportedSchema, ForcedToolChoice, UnsupportedPath }

pub struct Outcome { pub applied: bool, pub skip: Option<Skip> }

pub fn apply(req: &mut ChatRequest, enabled: bool, provider: &str) -> Outcome
```

When `enabled` is false, return `Skip::Disabled` and do not mutate `req`.

Call `apply` in `handlers/chat.rs` after `compress::apply` and before `brevity::apply`.
Pass `ctx.cfg.compact_tools_enabled` and `resolved.provider`.

### Tests

```rust
#[test]
fn unset_flag_leaves_the_request_byte_identical() {
    let mut req = sample_chat_request(); // tools: [calendar], stream: false, no tool_choice
    let before = serde_json::to_vec(&req).unwrap();
    let outcome = compact_tools::apply(&mut req, false, "openai");
    assert!(matches!(outcome.skip, Some(Skip::Disabled)));
    assert!(!outcome.applied);
    assert_eq!(serde_json::to_vec(&req).unwrap(), before);
}

#[test]
fn from_env_defaults_compact_tools_off_when_unset() {
    std::env::remove_var("COMPACT_TOOLS_ENABLED");
    assert!(!GatewayConfig::from_env().compact_tools_enabled);
}
```

`sample_chat_request` lives next to the test and builds a `ChatRequest` with one user
message and the router's `ToolDef`, not the crate's.
