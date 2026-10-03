# nasiko-tool-compact

Compact tool schemas for LLM requests, and a strict decoder for the calls a model writes back.

Pure library: no IO, no environment reads, no provider code, no dependency on `nasiko-llm-router`.

```rust
let compact = encode_tools(&tools)?;          // call-format instructions + one signature per tool
let calls = decode_calls(model_text, &tools)?; // validated against the ORIGINAL schema; never guessed
let mut d = StreamDecoder::new(&tools);        // incremental; handles markers split across chunks
let back = decode_tools(&compact)?;            // rebuild tool definitions to check meaning survived
```

Grammar, schema support and bypass rules: [`GRAMMAR.md`](GRAMMAR.md). Decoding is fail-closed: an error, never a repaired call.

## Run the checks

```sh
cargo test -p nasiko-tool-compact      # 46 tests
EVAL_SET=tool-compact/tests/fixtures/extra_cases.json OUT=/tmp/x.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
EVAL_SET=... OUT=/tmp/x.jsonl cargo run --release -p nasiko-llm-router --example compact_tools_report
cargo run --release -p nasiko-llm-router --example compact_tools_scaling
```

Live mode (optional): set `PROVIDER_BASE_URL` and `MODEL` (and `PROVIDER_API_KEY` if the endpoint needs one, read from the environment only); add `LIVE_NATIVE_BASELINE=1` to also send the native request for comparison.

## What is tested, and what is not

- Decoder: the public cases, our own 18 decoder cases, every two-chunk split of fixed texts, one character per chunk, randomized multi-chunk partitions, exactly-once emission, poisoning after an error, no panic on arbitrary input.
- Schema meaning: `decode_tools(encode_tools(T)) == T` for the sample tools and 2000 generated schemas.
- Robustness (decoder only): 64 grammar deviations are never repaired; 16,000 seeded random mutations never panic and never produce a call that fails the schema. This does NOT show that a model follows the format: live adherence is measured separately and is unmeasured until a live run is reported.
- Not covered: every possible partition of every input; constraint keywords (bypassed instead); Anthropic-style endpoints; streaming through the router.

## Limits

Token savings depend on the tool count and on the provider. Offline, measured with `o200k_base` on the complete request body (synthetic public-style tools, see the `compact_tools_scaling` header): compaction is larger than native below about 3 tools (such requests are sent natively), about 25% at 5 tools, about 34% at 10 and about 40% at 25 or more. The public sample has 2 tools (9%). On real models through a Nasiko router, billed prompt tokens fell 34% on Mistral Large 3 and did not fall on gpt-oss-120b, because Bedrock already renders native tools compactly there. Cost and latency savings are not claimed; they are not measured here. An independent reviewer's live run (gpt-4o-mini, Gemini 2.5 Flash, 7 compacted cases each) found adherence inconsistent: compact calls were valid for 2/7 versus 5/7 native on gpt-4o-mini, and 4/7 versus 3/7 on Gemini, with the decoder rejecting malformed markers; prompt tokens were higher with compaction on gpt-4o-mini and lower on Gemini. Treat the format as provider-dependent.

## Router wiring (opt-in)

`llm-router` can apply the codec to non-streaming chat completions. It is **off by default**; set `TOKEN_TOOL_COMPACT=true` on the server. With the flag off the request and response are byte-identical to before (`tool_compact::tests::disabled_leaves_the_request_byte_identical`).

When on, the router replaces native `tools` with a compact system message and turns the model's `<<call ...>>` reply back into standard `tool_calls`. It sends the request natively instead when: streaming; `tool_choice` is forced; the client set `parallel_tool_calls: false` or uses the legacy `functions`/`function_call` fields; the conversation already contains tool calls or results; a tool has extra fields (for example `strict`); any schema keyword is unsupported; or the compact text is more than 78% of the native tools JSON in bytes (a byte proxy, calibrated on 24 tool sets against exact token counts, with a narrow margin; re-derive it whenever the header changes). An invalid, unknown or truncated call returns a 502 error; nothing is guessed. Not covered: streaming, the Responses API handler, Anthropic-style endpoints as a provider.

Evidence of the effect: the `nasiko.tool_compact.native_bytes` / `compact_bytes` span attributes on the `gen_ai.chat` span, the `nasiko::llm_router::tool_compact` log line, and the provider-billed `prompt_tokens` recorded in token usage. Run the same request with the flag off and on and compare `prompt_tokens`.
