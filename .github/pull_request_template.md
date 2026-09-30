## Summary

<!-- What does this PR change and why? Keep it focused on one logical change. -->

## Related issue

<!-- e.g. Fixes #123 -->

## How I tested it

<!-- Describe formatting/lint/build/test evidence. Check all that apply. -->

- [ ] `cargo fmt` (clean)
- [ ] `cargo check --workspace` (zero warnings)
- [ ] `cargo clippy --workspace` (zero warnings)
- [ ] Unit tests: `just test-unit`
- [ ] Integration tests: `just test-server` / `just test-one <name>` (needs `just infra`)
- [ ] Manual verification (describe commands and output below)

```text
<!-- paste relevant commands and output -->
```

## Documentation updates

<!-- List updated docs, or explain why none are needed. -->

## Breaking changes

<!-- List any breaking API, CLI, config, or migration changes, or state "None". -->

## Checklist

- [ ] PR is focused on one logical change
- [ ] Tests added for new behavior where applicable; unit tests are hermetic
- [ ] No live credentials or secrets committed (`.env` remains gitignored)
- [ ] Description explains problem, approach, and follow-up work
