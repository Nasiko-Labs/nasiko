# Task 1 — Crate skeleton

Tracked in [`../TODO.md`](../TODO.md).

Scaffolding. No test cases.

Create `tool-compact/Cargo.toml` with `name = "nasiko-tool-compact"`, workspace edition
and version, and `serde`, `serde_json`, `thiserror` via `workspace = true`. No other
dependencies.

`lib.rs` enables `forbid(unsafe_code)` and denies `clippy::unwrap_used`,
`clippy::expect_used`, `clippy::panic`, and `clippy::string_slice`. Tests may use
`unwrap`. Declare private modules `types`, `grammar`, `encode`, `decode`, `stream`,
`schema`. Re-export the public functions. Until later tasks, each function returns
`Err(CompactError::UnsupportedSchema { name: "uninitialized".into(), feature: "not built".into() })`
and `StreamDecoder::push` returns `Ok(vec![])`.

Root `Cargo.toml`: add `"tool-compact"` to `members` and

```toml
nasiko-tool-compact = { path = "tool-compact" }
```

to `[workspace.dependencies]`.

Check: `cargo test -p nasiko-tool-compact` and
`cargo tree -p nasiko-tool-compact -i nasiko-llm-router` shows no reverse dependency.
