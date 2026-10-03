# Phase 2 tool selection

Phase 2 reduces the native tool catalog before the existing Phase 1 schema encoder.
It is a Rust library feature in the LLM router and an evaluator mode. The original
P1 grammar, argument validator, pure `tool-compact` crate, and official default
evaluator contract are preserved.

## Request and response behavior

Routing resolves from the original request. The selector then builds ordered
`tool_0000` IDs and bounded candidate metadata, uses independent Jev Noul questions,
validates the complete answer map, and applies local policy. Native tool definitions
are cloned by index, never generated or mutated by Jev. Mandatory and forced tools
are retained. Explicit dependencies are expanded to a fixed point; cycles terminate.

Optional candidates are ranked by probability, with ties broken by original order.
A candidate and its entire dependency bundle must fit the budgets before admission.
Mandatory tools and their closure can overflow the budgets; that overflow is reported.
Adding mandatory tools is monotone without budgets. With hard caps, mandatory additions
may displace optional tools. The specification's unrestricted monotonicity and hard-cap
requirements conflict in that case; mandatory retention takes priority.
Full-catalog failure fallbacks prioritize tool availability over configured budgets;
any resulting overflow is also reported. Choose deterministic-subset fallback when a
locally valid subset is preferable, or native fallback to preserve provider behavior.

After selection, Phase 1 compacts the complete selected catalog if it is supported.
If any selected schema is unsupported, the selected catalog stays native. Mixed native
and compact declarations are not introduced. Forced `tool_choice` keeps its native
provider semantics. Tool-result continuation turns are neither pruned nor compacted.

The current Phase 1 **router** compaction is non-streaming. Phase 2 can reduce the
native catalog for streaming requests, then uses the existing native provider stream.
The pure library's `StreamDecoder` remains tested at arbitrary chunk boundaries; this
change does not add a compact streaming gateway protocol. Non-streaming compact
responses still pass through Phase 1 decoding and validation, which reject unknown
or excluded tools and malformed arguments before normal ToolCall values reach clients.

## Jev contract and bounded state

The adapter reuses the router's `reqwest::Client` and posts to the configured base
URL plus `/v1/systemone` with bearer authentication. It sends only the latest user
text and candidate name, normalized description, coarse input summary, compactability,
and local cost. System messages, tool outputs, full history and full schemas are omitted.
Requests over 4,096 user-text characters fall back rather than dropping a subtask.
Descriptions are capped at 384 characters; input summaries at 512 characters and 16
fields; the external request is capped at 256 candidates and 256 KiB. Responses are
capped at 1 MiB and the timeout covers both headers and body reads.

The [official Noul documentation](https://docs.typesafe.ai/primitives/noul) states that
question IDs are not sent to the model. Each question therefore embeds its candidate
in structured instructions, rather than relying on an ID to identify a separate catalog.
Noul returns `noul` directly and has no separate confidence field. The adapter follows
the [documented HTTP interface](https://docs.typesafe.ai/api),
validates one typed answer per requested ID, rejects duplicate/foreign/missing IDs and
invalid probabilities atomically, and records the actual resolved model and usage when
available. Selection wording is versioned as `nasiko-noul-v1`.

Timeout, auth, rate-limit, service, transport, malformed-answer, mapping and policy
errors have distinct categories. There are no automatic retries. The configured
fallback runs once; if a deterministic fallback cannot produce a valid non-empty
subset, it retains all tools. Invalid local policy retains the full catalog. Malformed
dependency JSON disables selection at startup. Credentials have redacted Debug output;
upstream bodies, user text, schemas and tool names are excluded from default telemetry.

## Configuration

| Environment variable | Default | Behavior |
|---|---|---|
| `TOOL_COMPACTION_ENABLED` | false | Master switch; legacy `NASIKO_TOOL_COMPACTION` still works |
| `TOOL_SELECTION_MODE` | off | off, deterministic, jev, hybrid |
| `TOOL_SELECTION_FALLBACK` | compact-all | native, compact-all, deterministic-subset |
| `TOOL_SELECTION_THRESHOLD` | 0.5 | Include at or above this probability |
| `TOOL_SELECTION_UNCERTAINTY_FLOOR` | 0.35 | Retain uncertainty down to this floor; `off` disables it |
| `TOOL_SELECTION_MAX_TOOLS` | unset | Visible count cap except mandatory closure overflow |
| `TOOL_SELECTION_MAX_TOKENS` | unset | Sum of local per-tool bytes/4 estimates, not billing tokens |
| `TOOL_SELECTION_MIN_TOOLS` | 1 | Empty/undersized results invoke fallback |
| `TOOL_SELECTION_MIN_CATALOG_TOKENS` | 0 | Optional minimum estimated catalog cost before paying for Jev |
| `TOOL_SELECTION_TIMEOUT_MS` | 1500 | Total selector deadline |
| `TOOL_SELECTION_MANDATORY` | empty | Comma-separated native tool names |
| `TOOL_SELECTION_DEPENDENCIES` | empty | JSON object mapping native names to dependency-name arrays |
| `TYPESAFE_API_KEY` | unset | Required only when an external call is needed |
| `TYPESAFE_BASE_URL` | https://api.typesafe.ai | HTTPS base; HTTP allowed only for loopback tests |
| `TOOL_SELECTION_JEV_MODEL` / `TYPESAFE_MODEL` | jev-latest | Model alias or pinned version |

Deterministic mode is an intentionally simple lexical baseline. It can miss tools when
the user describes an intent without using catalog vocabulary, so it must be assessed
with recall, not token savings alone. Hybrid preincludes explicit native-name mentions
and mandatory tools, then asks Jev about the remaining candidates.

One-tool catalogs with a non-empty safeguard skip Jev because selection cannot shrink
them. `TOOL_SELECTION_MIN_CATALOG_TOKENS` can also skip small catalogs, retaining the
complete Phase 1 catalog with zero selector usage. Explicit count/token budgets take
precedence over that economic gate. Tune the gate and thresholds from representative
measured evaluations; no universal break-even or recall guarantee is claimed.

## Evaluation

The original command remains offline and byte-deterministic:

```sh
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release --locked -p nasiko-llm-router --example compact_tools_eval
```

Opt in with `EVAL_SELECTION_MODE=deterministic`, `jev`, or `hybrid`. This evaluator
switch is separate from deployment settings, so setting `TOOL_SELECTION_MODE=jev`
alone never makes the official P1 evaluator contact Jev. Model-provider generation is
still independently opt-in with `PROVIDER_BASE_URL` and `MODEL`.

Selection rows include selected names, actual available probabilities, telemetry,
required-tool recall/precision, false inclusions/exclusions, tool-count reduction,
full-request `o200k_base` counts, Phase 1 baseline counts, gross reduction and net
token effect. Ground truth comes from `required_tools`, or the supplied `expected`
calls, and is read only after selection. Decoder cases continue unchanged. Offline
deterministic rows omit variable timing and are reproducible byte-for-byte.

Net token effect subtracts actual Jev input plus output tokens from the full-request
saving. Jev's token counts and `o200k_base` are different accounting systems: this is
a reported token-count comparison, not a dollar-cost estimate. Missing usage produces
null net savings. A skipped call has measured zero selector usage. Offline roundtrip
validation is reported separately from model adherence; adherence remains null unless
a generating model is called. Live matching respects dataset `free_text_fields` and
compares call sets without depending on call order.

The public development sample is small and is not held-out leaderboard evidence.
Reducing tools does not imply a net saving or preserved recall. Inspect every row,
including failures, before choosing a deployment policy.

## Execute on a machine with Docker

From the repository root:

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
CARGO_NET_OFFLINE=true ./llm-router/scripts/compact-tools-eval.sh off
CARGO_NET_OFFLINE=true ./llm-router/scripts/compact-tools-eval.sh deterministic
# Export TYPESAFE_API_KEY in your shell for the live selector.
TOOL_SELECTION_TIMEOUT_MS=10000 ./llm-router/scripts/compact-tools-eval.sh jev
```

The runner defaults to `target/compact-tools-<mode>.jsonl`. Override `OUT` to save it
elsewhere. Initial builds need Rust/Docker and downloaded Cargo dependencies; omit
`CARGO_NET_OFFLINE=true` until the cache is populated. The completed evaluator can
execute without network in off/deterministic modes.

## Start the router

The existing standalone service exposes the feature on real chat requests. Point it
at Nasiko's already-migrated Postgres and use the existing identity/provider secrets:

```sh
export TOOL_COMPACTION_ENABLED=true TOOL_SELECTION_MODE=jev
# Supply DATABASE_URL, AGENT_JWT_SECRET, SECRETS_ENCRYPTION_KEY,
# TYPESAFE_API_KEY and the relevant provider key through your normal secret environment.
cargo run --release --locked -p nasiko-llm-router --bin llm-router
```

`LLM_ROUTER_BIND` defaults to `0.0.0.0:8081`; `GET /health` checks liveness. Agent chat
requests use `POST /v1/chat/completions` and a Nasiko agent identity JWT. The service
does not run database migrations. The generating provider key and TypeSafe selector
key serve different APIs. Existing Docker server images must be rebuilt from this
checkout to contain the changes; restarting an old image does not load new Rust code.

For Docker-only development, build the binary in the same Rust image/caches used by
the evaluator, then execute `target/release/llm-router` inside a container on the
database's network, forwarding the required environment variables by name. The
standalone binary is unchanged; database initialization and agent registration remain
Nasiko's existing deployment workflow.

```sh
docker run --rm -v "$PWD:/work" -w /work \
  -v nasiko-cargo-registry:/usr/local/cargo/registry \
  -v nasiko-cargo-git:/usr/local/cargo/git \
  rust:latest cargo build --release --locked -p nasiko-llm-router --bin llm-router
# This machine's existing Nasiko database is on the Docker network named nasiko.
# DATABASE_URL must address that network's Postgres service, not localhost.
docker run --rm -p 8081:8081 --network nasiko -v "$PWD:/work" -w /work \
  -e DATABASE_URL -e AGENT_JWT_SECRET -e SECRETS_ENCRYPTION_KEY \
  -e PLATFORM_OPENAI_API_KEY -e TYPESAFE_API_KEY \
  -e TOOL_COMPACTION_ENABLED=true -e TOOL_SELECTION_MODE=jev \
  rust:latest ./target/release/llm-router
```

## Validation and baseline

Before Phase 2 changes, the Phase 1 targeted suite passed: 384 router unit tests,
10 compact-library tests and 2 evaluator tests; environment-dependent tests were
ignored. The baseline commit was `133b3acd`. No new crates or network stack were added.

```sh
cargo fmt --all --check
cargo clippy --locked -p nasiko-tool-compact -p nasiko-llm-router --all-targets -- -D warnings
cargo test --locked -p nasiko-tool-compact -p nasiko-llm-router --lib --tests --example compact_tools_eval
# Optional, uses only environment-provided credentials:
cargo test --locked -p nasiko-llm-router live_jev_smoke --lib -- --ignored --nocapture
```

Tests cover bounded candidates, threshold boundaries, uncertainty, forced/mandatory
tools, budgets, dependency cycles/closure, registry safety, malformed/duplicate/missing
answers, all fallback categories, real HTTP wire shapes with mocks, timeout, hybrid,
economic skipping, original Phase 1 identity, native streaming preservation,
unsupported schemas, and provider-to-client compact call roundtrips.
