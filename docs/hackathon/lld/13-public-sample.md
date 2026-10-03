# Task 13 — Public sample

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

Same example, no new code except a bugfix the sample exposes. Download the published
file and run the command in the plan.

### Test cases

For every object in `cases` and `decoder_cases`, `OUT` has one line with the same `id`.

| Case | Assert |
| --- | --- |
| `ct-001` | `roundtrip_calls` has one call, name `create_calendar_event`, arguments equal `expected` as JSON values |
| `ct-002` | two calls, names match `expected` order; `subject`, `body`, and `title` are present and are strings; other fields match exactly |
| `ct-003` | `roundtrip_calls` is `[]` |
| a `dc` success, including the split marker | `decoded.calls` equals `expected.calls` |
| a `dc` error | `decoded.error` is `unknown_tool` or `invalid_arguments`, and `decoded.calls` is absent |

Relative split: take the rendered call string length `n` and the published chunk lengths
`c1..ck` whose sum is the published text length. Cut the rendered string at
`round(i * n / published_len)` for each chunk boundary. A test in `tool-compact` is not
required for the arithmetic; assert on the sample output that `dc` split-marker id
decodes to the expected call.
