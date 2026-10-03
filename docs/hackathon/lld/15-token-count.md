# Task 15 — Token count

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

In the example only, add `tiktoken-rs` pinned as a dev-dependency of `nasiko-llm-router`.
After writing `OUT`, if `COUNT_TOKENS=1`, count `o200k_base` tokens of each
`compact_request` and of a native body built from that case's tools and messages.
Savings for a `compacted: false` line are 0. Print one line:
`token_reduction=<fraction>`. Do not put `tiktoken-rs` on `nasiko-tool-compact`.

### Test case

Run the public sample with `COUNT_TOKENS=1`. Assert the process exits 0 and stdout
contains `token_reduction=` followed by a number `>= 0.30`. Keep the number for the PR.
A bypassed line in a hand-made fixture must not increase the numerator: two identical
native and compact bodies on a `compacted: false` case report `0` for that case.
