# [compact-tools] Compact function-tool schemas on the OpenAI path

## Track

P1, compact tool schemas. The work adds the `nasiko-tool-compact` crate and a seam in `llm-router`. Existing router behavior stays the default.

## How to run

`registry.nasiko.dev` returns 403 from this network, so the eval is not fetched. Use the vendored sample, which is the downloaded registry file.

sha256 `bc7e9dbdd91fc05ad73a9d6b46f30586c586d6b3305e3eb6d3353a87a0e899d9`

```sh
EVAL_SET=tool-compact/tests/fixtures/compact-tools-eval.json OUT=/tmp/out.jsonl \
  COUNT_TOKENS=1 \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

`COUNT_TOKENS=1` prints `token_reduction=` after `OUT` is written. `COMPACT_TOOLS_ENABLED` defaults off. `"true"` or `"1"` turns on the OpenAI non-streaming path.

The example is offline. It reads `EVAL_SET` from disk and does not call the local cluster. The cluster itself is deployed locally with Docker Compose at http://localhost:8080.

## Live mode

Live mode not run. No provider base URL and no model id were set.

## Token reduction

On the downloaded public sample (same bytes as `tool-compact/tests/fixtures/compact-tools-eval.json`), `o200k_base` reports `token_reduction=0.44359756097560976`. Two runs wrote the same 8 lines. A bypassed case adds nothing to the numerator.

Three extra sets were written and run locally. They are not the scored sample. Each run was repeated and the two `OUT` files matched.

| Local set | What it checks | Lines | `token_reduction` | Compaction |
| --- | --- | --- | --- | --- |
| Downloaded public sample | Calendar and email cases `ct-001`–`ct-003`, decoder cases `dc-001`–`dc-005` | 8 | 0.44359756097560976 | all three `ct` cases compacted |
| Nested visit | Nested object, number, boolean, split marker | 2 | 0.21774193548387097 | compacted |
| Notes | Two calls, a reply with no call, `>>` inside a string | 3 | -0.013157894736842105 | compacted |
| Bypass | Supported `ping` plus a `$ref` tool | 4 | -0.05785123966942149 | `ping` compacted, `$ref` left native |

The two small sets are negative because the compact request adds a system message (the fixed date line, the signature, and the call instruction). On a tiny schema that overhead can exceed the JSON Schema it replaces. The `$ref` case stays native, so its full request stays in the denominator and adds nothing to the savings. The scored public sample is the 44% figure.

## Known limits

Unsupported schemas stay native: `$ref`, `oneOf`, `anyOf`, `allOf`, `not`, non-boolean `additionalProperties`, `prefixItems`, tuple `items`, non-string enums, and any `format` other than `date-time`. One unsupported tool fails the whole batch.

Compaction runs only for OpenAI, non-streaming, when `tool_choice` is absent or `"auto"`. Streaming, Anthropic, Gemini, a forced tool choice, and the flag off all leave the request unchanged. A reply that does not decode is left as provider text. No tool call is invented.

No API keys, no `.env`, and no `OUT` file are in this diff.
