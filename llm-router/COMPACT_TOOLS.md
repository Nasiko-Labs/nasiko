# Compact tools contribution

Track: compact-tools. The pure codec lives in the nasiko-tool-compact crate. The
evaluation runner and experimental router adapter use the same encoder and decoder.

See [measured results and live limitations](COMPACT_TOOLS_RESULTS.md) and
[connection architecture](COMPACT_TOOLS_ARCHITECTURE.md). The live results show model-dependent reliability; the feature remains disabled by default.

## Run

    curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
    EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/compact-tools-out.jsonl \
      cargo run --release -p nasiko-llm-router --example compact_tools_eval

No required CLI arguments. Offline mode reads EVAL_SET and writes OUT, with no
HTTP client, credential read, network call, or model. Fetch dependencies ahead of time
for an isolated runner. OUT defaults to compact-tools-out.jsonl.

Normal rows contain id, compact_request, compacted, rendered_calls, roundtrip_calls.
Expected calls are rendered only for the required offline round-trip check; live
requests never include expected calls. Bypasses use native JSON serialization round
trips and are marked compacted: false. Decoder rows contain id and decoded, after
feeding the supplied chunks unchanged. The grammar accepts both <<NAME {JSON}>>
and the original <<call NAME {JSON}>>, plus explicit [TOOL_CALLS]NAME{JSON}
and [TOOL_CALLS]NAME({JSON}) aliases. Every original decoder case is still valid,
so no conversion or handwritten chunk positions are needed.

Output JSONL contains outputs, not scores. stderr reports aggregate request token
counts using exactly pinned tiktoken-rs 0.6.0 / o200k_base, an example-only
dev dependency. Counts include complete JSON bodies and call instructions.
Bypasses count as zero savings. Offline output excludes latency and is deterministic.

Live mode requires both PROVIDER_BASE_URL (a base ending in /v1, not the final
chat/completions URL) and MODEL. OPENAI_API_KEY is optional for proxies that supply
their own credentials. Credentials are never written to JSONL or logs.

    PROVIDER_BASE_URL=https://api.openai.com/v1 MODEL=gpt-4o-mini \
      EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/compact-tools-live.jsonl \
      cargo run --release -p nasiko-llm-router --example compact_tools_eval

Set OPENAI_API_KEY securely in the environment, never in a committed command or file.
Live mode adds raw_output, live_calls (calls or an error), model, usage, and
provider_finish_reason. Compact responses use the same validation/restoration
path as the router. Invalid native argument JSON becomes a per-case error rather
than aborting the rest of the evaluation. It uses
temperature 0 and a fixed system reference: 2026-10-02, Asia/Kolkata. Live outputs
may vary across providers/runs; offline determinism is the guarantee.

COMPACT_TOOLS_EVAL_MODE=compact (default) runs the proposed codec. Set it to native
to send the same messages/model/temperature with original tool definitions, for
an explicit live baseline. Native-mode rows are marked compacted: false and have
zero compaction savings. The default scoring command does not need this variable.

## Experimental router integration

COMPACT_TOOLS_ENABLED=true enables the adapter; the default is false.
No database migrations, protocol changes, provider-key changes, or frontend changes.
The request transformation runs after existing payload compression and brevity,
before provider dispatch and final request-size accounting. The original schemas
remain in per-request memory. Responses are validated before returning standard
tool_calls, with fresh IDs, string arguments, and finish_reason: tool_calls.
Real upstream usage is recorded even if decoding fails; validation failure returns
502 with a safe codec error code. There is no hidden retry.

Initial integration covers OpenAI Chat Completions inbound, the OpenAI outbound
provider, non-streaming, text messages, and auto/default tool choice.
Streaming requests, historical tool calls/results, multimodal content, strict or
future tool fields, additional request fields (including response_format), special
tool_choice, configured fallback chains, unsupported schemas, and requests with no
byte reduction bypass compaction before dispatch. Router streaming is not implemented;
the core incremental decoder is implemented and evaluated.

The adapter does not execute tools. The agent still executes its original tools,
including MCP tools. Ordinary text around decoded calls is preserved; invalid or
truncated calls never become executable. A complete response is checked before
any mutation, including across multiple choices. Truncated responses, missing/null text, and
unexpected typed native tool_calls in a compact response are rejected.

## Validation

    cargo test -p nasiko-tool-compact -p nasiko-llm-router

Library unit/property tests cover reversible schemas, types/enums/constraints,
escaping, duplicates, truncation, arbitrary chunks, and resource limits. Router
tests prove disabled serialized identity, unchanged bypasses, actual provider
request transformation, standard client response restoration, and unknown-call
rejection.

The public sample is vendored under tests/fixtures/compact-tools-eval.json for
offline integration tests. It is public development data, not a held-out set.
Thirty percent savings is a target; measured results belong in the PR, never an
assumed library guarantee. The organizers score the private set and live adherence.

Known public-sample inconsistency: ct-002 says "tomorrow" while the required live
reference date is 2026-10-02, but its expected timestamp is 2026-10-04. Live inference
must follow the fixed reference date; do not hardcode an exception for that case.

## Real execution demo

```sh
PROVIDER_BASE_URL=https://bedrock-mantle.us-east-1.api.aws/v1 \
  MODEL=mistral.devstral-2-123b \
  cargo run --release --locked -p nasiko-llm-router --example compact_tools_demo
```

Supply OPENAI_API_KEY securely in the environment. COMPACT_TOOLS_EVAL_MODE=native
runs the native baseline; DEMO_PROMPT optionally changes the request. The default
asks to sum [2,3,5] and count exactly "Hi 中文". Actual local CPU tools return sum
10, five Unicode scalar values, and nine UTF-8 bytes. The complete batch is schema
validated before execution. No tool response is simulated, no calls are invented,
and there is no hidden retry. The executions array shows what the model actually
requested; it does not assert that all requested actions were emitted.

This is a direct provider example sharing the router adapter. The deployed UI
assistant and its custom Bedrock route do not exercise compact-tools integration.
