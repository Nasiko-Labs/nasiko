# Task 14 — Deterministic eval

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

No new library code. Run the task 13 command twice with `OUT=/tmp/out-a.jsonl` and
`OUT=/tmp/out-b.jsonl`. Unset `PROVIDER_BASE_URL` and `MODEL`.

### Test case

`diff -u /tmp/out-a.jsonl /tmp/out-b.jsonl` prints nothing and exits 0.
