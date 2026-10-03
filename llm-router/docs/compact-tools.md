# Compact tools: eval and router integration

Track: compact-tools. This contribution adds `nasiko-tool-compact`, the required `compact_tools_eval` example and an off-by-default OpenAI chat transformation. It is independent of the classifier contribution.

## Run the submission

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
cargo fetch
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/compact-tools-out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

No arguments or credentials are required. `EVAL_SET` defaults to the bundled unmodified public sample; `OUT` defaults to `/tmp/compact-tools-out.jsonl`. Offline mode makes no network calls and reads no provider credentials. It renders expected calls from the dataset strictly for the prescribed offline codec round trip; this is not model-inference accuracy. Decoder cases are fed chunk by chunk, without reading their expected outcome.

One JSONL line per normal case contains `id`, `compact_request`, `compacted`, `rendered_calls` and `roundtrip_calls`. One per decoder case contains `id` and `decoded: {calls: [...]}` or `{error: code}`. No scores or `results.json` are written to `OUT`. Unsupported schema requests retain native definitions with `compacted:false`; their offline native roundtrip serializes/parses the expected calls directly rather than pretending compact decoding supports that schema. Bypassed decoder cases report `unsupported_schema`.

The example verifies `decode_tools(encode_tools(tools)) == tools` before claiming a compacted request. Token counts go to stderr using dev-only exact-pinned `tiktoken-rs = 0.7.0`, `o200k_base`, on serialized full request bodies. The library and production router do not depend on a tokenizer.

## Optional live run

```sh
PROVIDER_BASE_URL=https://your-proxy.example/v1 MODEL=your-chat-model \
  EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/compact-tools-live.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

Both endpoint and model must be set to enable live mode. The example appends `/chat/completions` to the base path unless already present. Optional `PROVIDER_API_KEY` (or `OPENAI_API_KEY`) supplies bearer auth; an unauthenticated organizer proxy needs neither. The example does not load `.env` automatically. Do not commit credentials or private evaluation outputs.

Live requests use temperature 0 and a separate system message: today is `2026-10-02`, timezone `Asia/Kolkata`. This reference-time message and live-only model/temperature settings are excluded from the reported offline token comparison, and included in each live `compact_request`. Each case gains `raw_output` and `live_calls: {calls: [...]}` or `{error: ...}`. Network/HTTP/parser failures produce error outputs; exit code 0 means the evaluation ran, not that it achieved a score. HTTP timeout is 45 seconds/case, responses are capped at 2 MiB and redirects are disabled. A shared 12-minute live deadline bounds repeated endpoint timeouts; remaining cases report `evaluation_timeout` after it expires.

## Router opt-in behavior

Set `COMPACT_TOOLS_ENABLED=true` in the existing gateway binary configuration (`src/config.rs`). Default is false. Only requests already passing through the router's OpenAI inbound `/v1/chat/completions` path are eligible, with resolved provider `openai`, no configured fallbacks, non-streaming output, supported function schemas and tool choice absent or `auto`.

The normalized IR is transformed before provider dispatch: inject the compact schema system message, remove native `tools` and `tool_choice`, and remove the now-inapplicable `parallel_tool_calls` option. Author messages are unchanged. After provider dispatch, validate all choices against original definitions and restore standard `tool_calls`, generated ids, JSON-string arguments and `finish_reason: tool_calls`. Prose accompanying calls is omitted, so compact markers never reach the client. Plain answers are unchanged.

Malformed output or schema violations return a 502 with a stable error code, without exposing model text/arguments or retrying a billed request. Provider usage is logged even when decoding rejects output. The trace logs that compaction applied without logging definitions or prompts.

Bypasses leave the normalized request byte-identical: disabled mode, streaming, other providers/inbound formats, configured fallback chains, forced/disabled tool choices, response-format constraints, unsupported schemas and prior tool-call/result history. Responses API is not wired. This intentionally small integration does not modify the gateway, orchestrator, provider credentials or default behavior.

## Measured public-sample results

On Apple M3 / macOS, using the published `compact-tools-eval-v1` sample:

| Check | Result |
|---|---:|
| Offline normal-case roundtrips | 3/3 exact matches |
| Stream decoder cases | 5/5 exact matches |
| Full native request tokens | 656 |
| Full compact request tokens | 477 |
| Reduction | 27.29% |
| Schema reconstruction | Exact for every selected tool |

Two offline runs are compared byte-for-byte. These three public cases are development evidence only. There is no private-set result. A live Bedrock run on `qwen.qwen3-coder-30b-a3b-instruct` decoded all three public responses, but only the plain-answer case exactly matched expected calls. The model added unspecified optional fields. `ct-002` also uses October 3 for tomorrow under the mandated October 2 reference, while the published expected call says October 4. These are limitations, not accuracy wins. No downstream task quality or production cost savings are claimed. Small tool catalogs can grow once the instruction overhead is included; this format does not guarantee savings for every request.

## Verify and demo

```sh
cargo fmt --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p nasiko-tool-compact -p nasiko-llm-router
```

The HTTP handler tests capture actual provider request bytes. They show the default native payload, opt-in compact definitions restored to standard calls, and unknown calls rejected without retries. The pure crate tests demonstrate split markers, escaping, nested schema constraints and atomic failure.

A short demo:

1. Run the required offline example. Show aggregate native/compact tokens on stderr.
2. Open `ct-001`: show the compact request, rendered call and original-name/argument roundtrip.
3. Open `dc-002` and `dc-003`: show split markers and `>>` inside a string both succeed.
4. Open `dc-004`/`dc-005`: show `unknown_tool`/`invalid_arguments`; no guessed call.
5. Run `cargo test -p nasiko-llm-router compact_tools_http` to show actual router behavior.

A judge can see Nasiko's role in 30 seconds: the router changes provider-facing tool definitions, validates compact output and restores the client's standard tool API. Agents need no protocol changes. Before broad adoption, measure live adherence across providers and real tool catalogs, strengthen schema/numeric coverage and optimize adversarial stream-chunk handling. Full streaming/fallback/history/forced-choice support remains future work.

## Live development checks

Bedrock Mantle endpoint: `https://bedrock-mantle.us-east-1.api.aws/v1`. Tested model: `qwen.qwen3-coder-30b-a3b-instruct`. The decoder explicitly accepts both `<<call NAME {JSON arguments}>>` and `<<NAME {JSON arguments}>>`, validating both against the same original schema. The renderer uses the published long form and official decoder chunks are fed unchanged. Reserved malformed `<<` markers fail instead of silently becoming empty calls.

Negative checks: `openai.gpt-5.6-luna` was listed by the endpoint but rejected on its Chat Completions route. `openai.gpt-oss-120b` returned reasoning-only/null-content responses for two tool cases. A Mistral Ministral 14B trial included malformed JSON. A placeholder OpenAI key produced HTTP 401. No accuracy or provider reliability claims are based on those trials. Successful parsing does not validate inferred dates or prevent schema-valid optional guesses; semantic task evaluation remains necessary.
