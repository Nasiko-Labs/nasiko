# Opt-in request classification

Nasiko already decides a model tier at safe routing boundaries. The regex baseline sees
only query keywords. This change separates **what work is requested and how complex it is**
from **which provider model to select**. Jev supplies the first decision; Nasiko owns
validation, fallback, provider tier mapping, learned quality cells and conversation stickiness.

## Run the scorer example

No arguments are required. With no configuration it runs the regex baseline offline against
the bundled validation split and writes `/tmp/classifier-out.jsonl`.

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/classifier-eval -o /tmp/classifier-eval.json
EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/regex-out.jsonl \
  cargo run --release -p nasiko-llm-router --example classifier_eval

# Supply CLASSIFIER_API_KEY securely in the process environment; do not commit it.
CLASSIFIER_BACKEND=jev CLASSIFIER_MODEL=jev-1.13.0 \
  EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/jev-out.jsonl \
  CLASSIFIER_INPUT_USD_PER_MILLION=0.042 \
  cargo run --release -p nasiko-llm-router --example classifier_eval
```

The example does not load `.env` automatically. A configured egress proxy may hold the key:
an empty `CLASSIFIER_API_KEY` omits the Authorization header. The hosted endpoint must speak
the [Typesafe System One API](https://api.typesafe.ai/redoc), not Chat Completions.

`OUT` contains exactly one output row per input case:

```json
{"id":"case-1","request_type":"writing","complexity":2,"confidence":0.94,"latency_us":350000}
```

It never contains labels or scores. Aggregate local measurements go to stderr. Optional
`CLASSIFIER_DIAGNOSTICS=/tmp/diagnostics.jsonl` writes case IDs, backend candidates, fallback
status and model ID separately, without prompt text. Exit zero means the evaluation ran,
including a run where the backend failed and every decision fell back. A bad input/output
file fails with a nonzero exit.

## Configuration

The existing `GatewayConfig::from_env` composition boundary reads these once. The library
backends and decision path never read environment variables or files. The router holds a
shared runtime containing `Arc<dyn RequestClassifier>`; the example constructs the same runtime.

| Variable | Default | Meaning |
| --- | --- | --- |
| `CLASSIFIER_BACKEND` | `regex` | `regex` or `jev`; unsupported values yield counted setup fallbacks |
| `CLASSIFIER_ENDPOINT` | `https://api.typesafe.ai/v1/systemone` | Complete POST endpoint; HTTPS, or loopback HTTP for tests |
| `CLASSIFIER_MODEL` | `jev-latest` | Use an available pinned ID for reproducible experiments |
| `CLASSIFIER_API_KEY` | empty | Bearer credential; never logged |
| `CLASSIFIER_TIMEOUT_MS` | `2000` | Whole decision timeout, including HTTP and response decoding |
| `CLASSIFIER_MIN_CONFIDENCE` | `0.6` | Minimum selected request-type probability |
| `CLASSIFIER_MIN_COMPLEXITY_CONFIDENCE` | `0.4` | Minimum probability of the selected discrete effort level |
| `CLASSIFIER_SEED` | `42` | Opt-in Thompson tier-selection seed; does **not** seed hosted inference |

Eval-only `CLASSIFIER_INPUT_USD_PER_MILLION` supplies a price assumption for local cost
reporting. No pricing claim or API fee is hard-coded into the runtime.

## Decision and routing policy

`RequestClassifier::classify(query, context)` returns one of the existing seven request
types, effort 1–5, and confidence. Regex wraps the original query-only function unchanged:
its fixed complexity is 3 and confidence is 0, an **uncalibrated placeholder**. Its complexity
never imposes a new tier floor in default mode. Default routing retains entropy-driven
Thompson exploration, weights, priors, provider registry and feedback behavior.

Jev receives two independent questions in one POST: a seven-way Choice and a five-level
Score. Request type follows the primary deliverable, rather than incidental words. Code
modification is `code_generation`; interpretation of supplied implementation is
`code_understanding`; a design deliverable is `technical_design`; causal/data reasoning is
`analytical_reasoning`; prose composition is `writing`; definitions/reference facts are
`factual_lookup`; remaining social/personal requests are `general`.

Effort: 1 trivial single action; 2 straightforward; 3 bounded multi-step; 4 substantial
reasoning/design; 5 intricate cross-component reasoning and validation. The exact rubric
is versioned in `routing/jev.rs`. Score levels start at zero on the wire. We select the most
probable level plus one, preferring the harder level on an exact tie; we do not round the
fractional expected score. Confidence is the selected **request-type probability**, not an
unverified vendor calibration claim. Complexity certainty is separate.

Responses must have valid answer types, known labels, complete finite distributions in
[0,1], approximate sum 1, consistent selected choice, the original rubric, a consistent
expected score, model ID and unsigned token usage. Malformed responses fail closed.
Response bodies are bounded to 64 KiB, redirects are disabled, and errors omit bodies,
credentials and endpoints.

| Situation | Behavior |
| --- | --- |
| Default `regex` | Original query label and tier math, no hosted call |
| Confident opt-in decision | Seeded original cost/quality selection with capability floor |
| Setup, transport, HTTP, response error or timeout | Query-only regex classification; seeded original tier math, no complexity floor; counted fallback |
| Low type or complexity certainty | Regex fallback output, keep agent-configured model without pinning; counted fallback |
| Pinned agent, cache hit, `continue`, pinned flow, missing conversation/config/query | Skip decision backend |
| Tier mapping unavailable | Use configured model |

Only complexity 1–2 permits all tier arms; 3 excludes Tier3; 4–5 permits Tier1. These are
conservative **hypotheses**, not demonstrated optimal cost/quality mappings. Cheap/simple
does not force Tier3. Provider-specific registry mappings and per-agent overrides still
choose concrete models. Provider credentials and selection remain the resolver's responsibility.

Opt-in selection seeds the existing RNG from configured seed, agent, provider, conversation,
query and context. For identical inputs, classifier output, learned cells and binary,
selection is reproducible. Feedback cells remain `(provider, tier, request_type)`; complexity
does not create new bandit keys, and no simulated feedback is used in accuracy evaluation.
Cached decisions preserve the chosen tier/model. Opt-in transcript-derived tool continuation
guards ordinary flows even when their existing boundary derivation says `switch`.

Hosted input is bounded to 16,384 query and 8,192 context Unicode characters. Router context
uses recent user/assistant text (most recent first) and existing A2A history prefixes,
excluding separate system/developer messages and tool payloads. Context outside those bounds
can be lost; this is an explicit limitation, not a claim of complete transcript understanding.

## Labelled data and development method

`tests/data/classifier_train.json` has 32 authored development cases;
`classifier_validation.json` has 35 separate cases. Both contain query, context, label,
complexity, a scenario-family ID and rationale. These are synthetic single-author provisional
annotations, not private customer data or independently adjudicated ground truth.

We developed the label criteria on the development split, compared the regex and Jev
outputs there, and compared complexity certainty cutoffs 0.4/0.5/0.6 while holding the type
cutoff at 0.6. We froze criteria and chose 0.4 before evaluating validation. This favored
request-type coverage at the cost of some complexity accuracy. Jev was **not fine-tuned**:
"train" means task-specification/threshold development for a hosted pretrained backend.
The brief allows hosted models but does not explicitly resolve whether this interpretation
of the training requirement is accepted; organizer confirmation is still needed.

Scenario inputs are independently authored rather than generated by paraphrasing a common
template. Greeting, acknowledgement and leisure-recommendation families were removed from
training during a semantic-overlap audit. Automated tests guard label validity, family/ID
disjointness and exact text duplication; human review remains necessary for semantic
near-duplicates and ambiguous labels. No official sample answers are fed to the backend.
The 10 public cases are a contract smoke test, not evidence of generalization. The small
local split cannot establish performance on the private scoring set.

## Measured results and limitations

Runs on 2026-10-03, macOS ARM64, Apple M3, 16 GiB RAM, Rust 1.99.0. Hosted model
`jev-latest` resolved to `jev-1.13.0`; final experiments pinned that ID. Measurements include
network, input bounding, validation and fallback; one-time regex/backend setup is excluded.
Results below are local and will not decide the private ranking.

| 35-case validation | Regex | Jev + fallback |
| --- | --- | --- |
| Request-type correct | 10/35 (28.6%) | 33/35 (94.3%) |
| Complexity correct | 7/35 (20.0%; fixed 3) | 27/35 (77.1%) |
| Complexity mean absolute error | 1.20 | 0.257 |
| Fallbacks | 0 | 2/35 |
| p50 / p95 decision latency, release | 4 / 79 µs | 314 / 406 ms |
| Construction time, same runs | 9.2 ms | 8.2 ms |
| Estimated decision API cost | $0 | $0.0000371 |

Jev reported 30,921 input tokens for 35 cases. Cost assumes the published
[$0.042 per million input tokens, free output](https://typesafe.ai/blog/introducing-system-one-models-and-jev)
on the run date; no local-hardware dollar cost is estimated. Usage can be missing for failed
requests, so reported usage does not promise complete billing attribution. The diagnostic
ECE including fallback placeholders was 0.069 (10 equal-width bins); regex/fallback confidence
0 is uncalibrated, so this is not a calibration guarantee. A second pinned run kept all
labels and complexity levels but changed confidence on 7/35 cases.

Negative examples: a contextual database query-plan review was confidently classified as
analytical reasoning instead of code understanding; an unsafe-code audit and a constrained
picnic-planning request deferred to regex. Complexity labels also disagree on borderline
levels. We made no further changes based on these validation outcomes.

**Unresolved determinism:** exact hosted predictions are not reproducible. Initial development
runs also changed one fallback decision. The published API exposes no inference seed.
`latency_us` records real wall-clock timing and necessarily differs between runs. Do not claim
byte-identical OUT or hide variance by inventing timings/confidence. Harness clarification
and a deterministic hosted option are needed before this meets that submission condition.

No downstream answer quality, model usage or workload savings evaluation was performed;
classifier accuracy does not demonstrate cheaper or better answers. No local model backend,
model training pipeline, flow decisions, agent routing or tool preselection is included.
Cache loss/restart can still fall through to the configured model during continuation,
as in the existing router; skipping reclassification cannot recover missing sticky state.

## Reproduce the 30-second demo

```sh
# Run offline first, then run with the same Jev env vars as above.
cargo run --release -p nasiko-llm-router --example classifier_routing_demo
```

The judge runs a tiny source edit and a complex recovery-design request. The **actual Nasiko
routing function** selects a provider tier; a continuation reuses the selected model with
zero additional classifier calls. An explicitly labelled injected timeout demonstrates
regex fallback. Provider model names are illustrative configuration overrides, and this
demo makes no answer-model calls. `classifier_eval` supplies aggregate measured metrics,
successes and the documented real failures.

## Checks

```sh
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p nasiko-llm-router --lib
cargo test -p nasiko-llm-router --test classifier_data
```

Unit tests cover HTTP request shape and validation, uncertainty/errors/timeout, default
behavior, provider overrides, reproducible tier selection, safe boundaries and sticky
continuation. A real outbound request capture proves the default tool transcript remains
byte-identical. HTTP mocks run only on loopback and need no provider keys/DB/Redis.
