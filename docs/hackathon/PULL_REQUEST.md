# [compact-tools] Compact function-tool schemas on the OpenAI path

## Track

P1, compact tool schemas. The work adds the `nasiko-tool-compact` crate and a seam in `llm-router`. Existing router behavior stays the default.

## How to run

```sh
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

`COUNT_TOKENS=1` prints `token_reduction=` after `OUT` is written. `COMPACT_TOOLS_ENABLED` defaults off. `"true"` or `"1"` turns on the OpenAI non-streaming path.

## Live mode

Live mode not run. No provider base URL and no model id were set.

## Token reduction

On the vendored public sample (`tool-compact/tests/fixtures/compact-tools-eval.json`), `o200k_base` reports `token_reduction=0.44359756097560976`. A bypassed case adds nothing to the numerator.

## Known limits

Unsupported schemas stay native: `$ref`, `oneOf`, `anyOf`, `allOf`, `not`, non-boolean `additionalProperties`, `prefixItems`, tuple `items`, non-string enums, and any `format` other than `date-time`. One unsupported tool fails the whole batch.

Compaction runs only for OpenAI, non-streaming, when `tool_choice` is absent or `"auto"`. Streaming, Anthropic, Gemini, a forced tool choice, and the flag off all leave the request unchanged. A reply that does not decode is left as provider text. No tool call is invented.

No API keys, no `.env`, and no `OUT` file are in this diff.
