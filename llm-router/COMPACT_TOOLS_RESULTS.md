# P1 validation results

Measured 2026-10-03 with Rust 1.94.1 and locked dependencies. This is an experimental
contribution. COMPACT_TOOLS_ENABLED remains false by default; the running server
has not been rebuilt or restarted with this contribution.

## Offline correctness and request size

```sh
EVAL_SET=llm-router/tests/fixtures/compact-tools-eval.json OUT=/tmp/compact-tools-out.jsonl \
  cargo run --release --locked -p nasiko-llm-router --example compact_tools_eval
```

| Dataset / mode | Normal + decoder rows | Baseline tokens | Output request tokens | Reduction | Bypasses |
| --- | ---: | ---: | ---: | ---: | ---: |
| Organizer public sample / compact | 3 + 5 | 656 | 493 | 24.85% | 0 |
| Author supplementary / compact | 3 + 1 | 769 | 631 | 17.95% | 0 |
| Author broader sample / compact | 13 + 9 | 2134 | 1857 | 12.98% | 5 |
| Organizer public sample / native | 3 + 5 | 656 | 656 | 0% | 3 |

All 34 offline rows match the expected roundtrip calls or decoder result. A repeated
public run is byte-identical. Native-mode normal rows are compacted: false.
Pinned tiktoken-rs 0.6.0 / o200k_base counts the complete serialized request,
including instructions. Decoder-only rows contribute no request tokens. Bypasses
have zero savings. Author fixtures are development data, not a held-out benchmark.
The public serialized-request reduction remains below the 30% target.

410 tests passed with zero failures: 389 router, 18 codec unit/property, two demo,
and one evaluation-example test. One router and six existing fixture-suite tests
remain ignored. Final scoped Clippy with -D warnings and changed-file formatting checks pass.
A separate fresh upstream checkout with only this contribution passes
cargo check --workspace --locked. The exact release evaluation command also passes from that checkout and produces
byte-identical public output. A whole-workspace fmt check in the fresh checkout
finds existing formatting in untouched llm-router/tests/router_e2e.rs; that file
is excluded from this P1 patch. Runtime services and UI are outside the patch.

## Live comparison: two model families

Real requests used the Bedrock OpenAI-compatible endpoint, temperature 0, and the
required 2026-10-02 Asia/Kolkata reference date. Expected calls never enter live
requests. Models: mistral.devstral-2-123b and qwen.qwen3-coder-30b-a3b-instruct.
These are two model vendors behind one endpoint, not two independently operated
provider endpoints. Each cell represents one small development run; temperature 0
does not guarantee reproducibility.

| Model / dataset / mode | Exact normal cases | Parse/validation errors | Provider prompt tokens |
| --- | ---: | ---: | ---: |
| Mistral / public / compact | 1/3 | 0 | 560 |
| Mistral / public / native | 1/3 | 0 | 934 |
| Mistral / broader / compact | 13/13 | 0 | 2199 |
| Mistral / broader / native | 13/13 | 0 | 3010 |
| Qwen / public / compact | 1/3 | 1 | 543 |
| Qwen / public / native | 1/3 | 1 | 1785 |
| Qwen / broader / compact | 8/13 | 5 | 3227 |
| Qwen / broader / native | 12/13 | 0 | 6005 |

Broader compact Mistral includes eight transformed requests and five native
bypasses. All 13 match calls/arguments exactly, including empty-call cases. Of
Qwen's eight transformed broader requests, three match exactly and five fail;
all five native bypasses match exactly. Native Qwen adds optional filters in one
case, causing an exact mismatch. One public native Qwen response has invalid
argument JSON, reported as a per-case error without aborting later cases.

Mistral emits all requested public actions, but invents optional defaults and
capitalizes/paraphrases titles. In ct-002, both families' valid compact calendar
calls use 2026-10-03 for tomorrow, while the fixture expects 2026-10-04. Under the
mandated 2026-10-02 reference, 10-03 is correct; no case-specific correction is
implemented. These explain why action coverage is not exact-label success.

Qwen failures include key=value arguments, extra JSON braces, incomplete closing
markers, and a search call with missing required filters on an unsupported action.
They are rejected; the codec never repairs JSON or guesses missing arguments.
An ordinary prose response may legitimately decode to no calls even if the model
omitted an action. Schema validation cannot establish semantic intent.

Live serialized-body counts differ from provider usage:

| Model / dataset | Serialized native -> compact | Reduction |
| --- | ---: | ---: |
| Mistral / public | 806 -> 649 | 19.48% |
| Qwen / public | 818 -> 661 | 19.19% |
| Mistral / broader | 2784 -> 2523 | 9.38% |
| Qwen / broader | 2836 -> 2575 | 9.20% |

Provider prompt_tokens above include any cached tokens reported by the endpoint.
They are returned by the endpoint, not independently verified billing totals.
Different tool accounting and templates mean serialized size and provider usage
must remain separate. Token reductions with invalid/missing calls are not evidence
of a usable optimization.

## Real execution demo

The compact_tools_demo example sends a real hosted-model request, validates the
complete returned batch, then executes local sum_numbers and count_text CPU tools.

| Model / mode | Actual default-prompt execution | Provider prompt tokens |
| --- | --- | ---: |
| Mistral / compact | Sum 10; Unicode scalar count 5; UTF-8 bytes 9 | 219 |
| Mistral / native | Same two tool results | 289 |
| Qwen / compact | Sum 10; count_text omitted by the model | 219 |
| Qwen / native | Sum 10; Unicode scalar count 5; UTF-8 bytes 9 | 537 |

Mistral compact executes both tools with 24.22% lower reported prompt usage in this
run. Qwen's 59.22% reduction accompanies a missing action and is not a successful
complete task. No synthetic response or hidden retry is used. An earlier Qwen run
executed both tools, illustrating run-to-run variance; this table uses the final
run only. The demo directly shares the router adapter rather than passing through
the deployed UI/custom-provider route.

## Remaining limits before activation

- Improve generic action fidelity across families and on unseen schemas; Qwen is
  currently unreliable. Do not weaken validation to chase sample scores.
- Reach and validate the 30% serialized-request target while preserving complete
  actions. Current measurements do not establish a private-set score.
- Build a tool-enabled UI agent demo and add custom-provider integration if needed.
  The existing runtime remains unchanged. Router SSE, history rewriting, multimodal
  requests, special tool choice, and fallback chains bypass compaction.
- Final organizer/private evaluation remains outstanding.

Raw public/author JSONL and demo outputs are retained locally under
.local/p1-results/final-2026-10-03/ (ignored), without credentials or headers.
Earlier initial OpenAI experiments had missing actions and poor savings. Current
catalog/instruction changes remove the version marker from the model prompt and
explicitly support valid Mistral textual frames; they do not prove those earlier
OpenAI families are now reliable.
