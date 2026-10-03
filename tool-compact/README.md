# nasiko-tool-compact

Compact tool schemas and a decoder back to standard tool calls. Grammar: `GRAMMAR.md`.

## Eval

The example is `llm-router/examples/compact_tools_eval.rs`. It does not read `COMPACT_TOOLS`. That flag is the router switch and defaults off.

Offline (no network, no key). Run it twice and diff `OUT`; the file must match.

```sh
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

Live mode runs only when both `PROVIDER_BASE_URL` and `MODEL` are set. `OPENAI_API_KEY` is optional. If set, it is sent as `Authorization: Bearer` and is not written to `OUT`, stderr, or any file.

`PROVIDER_BASE_URL` is an OpenAI-compatible root, usually ending in `/v1`. The example POSTs `{PROVIDER_BASE_URL}/chat/completions` (it does not append that suffix if the base already ends with it). The JSON body is `compact_request`: `temperature` 0, `model` from `MODEL`, and `messages`. The first message is `role: system` and starts with `Today is 2026-10-02. Timezone: Asia/Kolkata.` When compaction succeeds, the tool definitions are text in that system message and the body has no `tools` field. When a schema cannot be compacted, the native `tools` array is sent instead.

A 401, 429, 5xx, timeout, empty body, or missing content is `live_calls.error = request_failed` for that case. The example keeps going and exits 0. Offline failures (missing `EVAL_SET` or `OUT`, bad JSON) exit non-zero.

```sh
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  PROVIDER_BASE_URL=https://example.invalid/v1 \
  MODEL=your-model \
  OPENAI_API_KEY=your-key \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

Set `OPENAI_API_KEY` in the environment. Do not commit it.
