# nasiko-tool-compact vs the state of the art

This compares the `aggressive-compression` branch of `nasiko-tool-compact` with the solutions
surveyed in *P1: Compact Tool Schemas — SOTA Landscape, Gaps, and Build Strategy*.

Every figure is labeled as one of:

- **Measured**: run in this repo with the o200k_base tokenizer.
- **Reported**: the project's own claim, as quoted in the P1 survey. Not reproduced here.

## How it was measured

- **Eval set**: `reference/compact-tools-eval.json`. It has 3 cases using 1–2 tools each,
  and the cases include multiple calls and plain text.
- **Metric**: the one used by `compact_tools_eval`. It is the whole JSON request body with
  native `tools`, compared with the same body with the compact block in the system message.
  Baseline: 728 tokens.
- **Real tools**: the 47 tool definitions from the 9 agents in `agents/*/src/tools.rs`.
  Only the tool segment is counted.
- **Other formats**: rendered by a scratch harness that uses the same tokenizer and
  request shape.

## Headline comparison

| Approach | Token reduction | Source | Fails closed on bad calls | Streaming decoder | Provider-agnostic | Helps with 1–2 tools |
|---|---|---|---|---|---|---|
| **nasiko-tool-compact, aggressive branch** | **46.4%** eval set, **32.2%** real agent tools | Measured | Yes | Yes | Yes | Yes |
| nasiko-tool-compact, safe version | 30.5% eval set, 12.0% real agent tools | Measured | Yes | Yes | Yes | Yes |
| P1 survey §20 signature format, structure only, one-line | 54.7% | Measured (our rendering of their format) | Not specified | Not specified | Yes | Yes |
| P1 survey §20 signature format, structure only, multi-line as written | 44.5% | Measured (our rendering of their format) | Not specified | Not specified | Yes | Yes |
| Our format, structure only | 54.7% | Measured | Yes | Yes | Yes | Yes |
| TSCG and other deterministic schema compilers | 50–72%; one study reports 44–50% | Reported | Not stated | Not stated | Yes | Yes |
| Python signatures (Gorilla and NexusRaven style) | 20.7% | Measured (our rendering) | n/a | n/a | Yes | Yes |
| OpenAI's internal TypeScript namespace (gpt-oss harmony) | 16.6% | Measured (approximation) | n/a | n/a | No | n/a |
| Hermes/Qwen style: JSON schemas inside `<tools>` | about 0% (this is the baseline) | Measured | n/a | n/a | Yes | No |
| Anthropic Tool Search | about 85% on large catalogs | Reported | n/a | n/a | No | No |
| OpenAI Tool Search, OpenRouter, embedding filters | Grows with catalog size | Reported | n/a | n/a | Partly | No |
| BAML | No figure for tool schemas | Reported | No (recovers and coerces) | Yes | Yes | Yes |
| Code Mode, code2mcp, Programmatic Tool Calling | Very high on huge API surfaces | Reported | n/a | n/a | No (needs a sandbox) | No |

"n/a" means the approach does not decode compact calls, so the property does not apply.

## Where we are better

### 1. Savings on small tool sets, where tool search saves nothing

Tool search, deferred loading and embedding filters cut the *number* of tools. With 1–2 tools
per request there is nothing to cut, so they save about 0%. Per-tool compaction is what still
works there, and we get **46.4%** on exactly that case.

We also avoid their main failure mode. A retrieval miss hides the correct tool, and then no
model can call it. Every tool we are given reaches the model.

### 2. Rejecting invalid calls instead of repairing them (compared with BAML)

BAML's schema-aligned parsing may repair or coerce invalid output. We never do that:

- Each of the following is an error, never a guessed call: unknown tool, unknown key in a
  closed object, missing required field, wrong type (`1.0` for an `int` is rejected), enum
  value not in the list, duplicate JSON key, malformed or unterminated call.
- **Tests:** `i4_invalid_or_malformed_is_never_a_call`, and
  `fuzz_mutations_never_yield_bad_calls`, which mutates a valid call one character at a time.
  In the eval, `dc-004` (unknown tool) and `dc-005` (invalid arguments) are rejected.
- **In the router:** an invalid reply is re-sent natively before the client sees anything.

### 3. Proof that compaction did not change the schema (compared with TSCG-style compilers)

The survey's main worry about compilers is that more compression can quietly change the
schema. Every `encode_tools` call checks its own output:

- The block is decoded back.
- It must match exactly what was meant to be shown.
- With descriptions removed, it must be identical to the original schema. Types, required
  fields, enum values, defaults and closed objects all have to survive.
- If any check fails, the request is sent with native tools.

So the only lossy step, description trimming, is limited by construction to descriptions.
The survey's Aggressive profile asks for exactly this: "must be explicitly documented".

### 4. Smaller than the survey's own recommended format

On structure alone, our syntax ties the survey's §20 signature on one line (54.7% each), and
saves **10 points more** than §20 laid out on multiple lines as written (44.5%).

The main reason is that required is the default. Only optional fields carry a mark (`?`),
instead of `!` or `?` on every field. With descriptions included, we reach **46.4%**. The best
§20 variant reaches **36.7%**, because our description trimming applies on top.

Caveat: §20 does not say where descriptions go, so that variant uses our own placement.

### 5. Streaming that has been tested on every chunk boundary

The survey's Gap 4 asks for markers split across chunks and `>>` inside JSON strings to be
handled. `i6_any_chunking_equals_whole_decode` feeds inputs split at every character position,
plus 300 random multi-way splits, and checks the result is identical to decoding the whole
input. Memory is bounded: at most a 5-byte marker prefix, or one call up to 1 MiB.

### 6. Coverage measured on real tools, with explicit fallback

Other projects publish coverage claims. We measured ours on the real agent tools:

- Before the `default` and quoted-enum changes, 5 of the 9 agent tool sets fell back to native.
- After them, all 9 compact, and savings on real tools went from 12.0% to 32.2%.
- Anything still unsupported is rejected with the field path and reason, and the request goes
  out natively. Nothing is approximated.

### 7. Same output on every run

The same tools always produce the same bytes, regardless of JSON key order or `required` order
(test `i7_same_tools_same_bytes`). The eval output was byte-identical across two runs.

The survey lists this as Gap 6: a stable prompt prefix is what makes provider prompt caching
effective.

### 8. Standard JSON arguments, as the survey recommends

The schema is compacted; the call arguments stay plain JSON. They are parsed by `serde_json`
over an exact byte span found by a string- and escape-aware scanner, with no regex and no
second argument format.

## Where we are not better yet

| Area | Status |
|---|---|
| **Live adherence across model families (Gap 7)** | Run on **one** model so far: `anthropic/claude-sonnet-5.5`, 3 of 3 cases correct in format, including two calls in one reply (ct-002) and plain text (ct-003). Other model families are still untested. |
| Raw compression compared with TSCG | TSCG reports 50–72%. We measure 46.4% on the eval set and 32.2% on real tools. Our structure-only figure, 54.7%, is inside their range. Their numbers come from different benchmarks and are not reproduced here. |
| Schema coverage | `null`, `anyOf`/`oneOf`/`$ref`, numeric and string constraints (`minimum`, `pattern`, …) and non-scalar defaults still fall back to native. The survey's suggested matrix supports `null` and some constraints. |
| Compression profiles (Gap 8) | Safe and aggressive are separate branches, not a runtime setting. |
| Streaming in the router | The crate's `StreamDecoder` is complete and tested, but the router only compacts non-streaming requests today. |
| Eval size | 3 cases and 2 tools. The 46.4% depends on one rule (treating "user" as a filler word); without it the figure is 43.1%. |

## Scorecard against the P1 survey's gaps (§18)

| Gap | Status | Evidence |
|---|---|---|
| 1. Per-tool compaction for small and medium tool sets | Done | 46.4% with 1–2 tools; 32.2% across 47 real tools |
| 2. Compact representation and strict decoder as one layer | Done | `encode_tools` → `decode_calls` / `StreamDecoder` → `validate_arguments` |
| 3. Explicit supported-schema subset | Done | Capability matrix in the README; unsupported means native fallback |
| 4. Incremental streaming decoder | Done in the crate; not used for streaming in the router | `i6_any_chunking_equals_whole_decode`, `hold_back_is_bounded` |
| 5. Strict fail-closed validation | Done | `i4`, fuzz tests, dc-004 and dc-005 |
| 6. Deterministic, cache-stable encoding | Done | `i7`; byte-identical eval runs |
| 7. Model adherence benchmark | Partly | Live run on claude-sonnet-5.5: 3/3 cases in format; one model family only |
| 8. Compression profiles | Partly | Safe and aggressive exist as branches, not a runtime setting |
