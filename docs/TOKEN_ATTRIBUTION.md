# Token Attribution via `traceparent`

How the platform answers "which user burned these tokens?" for every LLM call, across
any agent language. Developer-facing contract lives in
[AGENT_LIFECYCLE.md](AGENT_LIFECYCLE.md); this is the design.

## The identity model

Two identities ride on every agent→LLM call; neither is optional — a call that
cannot present both is **rejected**, not served (see "Strict enforcement" below):

| Identity | Carrier | Set by |
|---|---|---|
| **Agent** | `Authorization: Bearer <jwt>` — minted at deploy, injected as `OPENAI_API_KEY` | platform |
| **User/session** | `traceparent` header — W3C trace context whose **trace_id is the flow id** | agent (forwarded from its inbound request) |

The LLM router verifies the JWT to know *which agent* is calling. The `traceparent`
trace_id names a row in the `flows` table — written synchronously by the agent proxy
**before** the request reaches the agent — which carries `user_id` and
`metadata.context_id`. Joining the two yields per-user, per-session token usage with no
agent cooperation beyond header forwarding.

```
user ──► /api/orchestrator/a2a ──► agent_proxy writes flows row (user_id, context_id)
                                        │  forwards request + traceparent
                                        ▼
                                     agent ──► LLM router (JWT + traceparent)
                                        │        │
                                        │        ▼
                                        │   attribution::resolve:
                                        │     trace_id → flows row → user_id, context_id
                                        ▼
                                   token_usage row (user_id, session_id, attribution source)
```

## Attribution resolution (`llm-router/src/routing/attribution.rs`)

There is exactly one path — **`traceparent`, resolved, or rejected**:

1. The header's trace_id is looked up in `flows`. Billing:
   `token_usage.user_id` = the flow's **caller** (`flows.user_id`), never the
   agent's owner — an agent serving another user's traffic bills that user.
2. The JWT-authenticated agent must be a **participant** of that flow (the flow was
   dispatched to it). This binds the two identities: without it, agent A could name
   agent B's flow and drain another user's token budget or misbill them.
3. Anything else — header absent, malformed, trace_id with no live `flows` row, or a
   flow the calling agent is not part of — is rejected with `403` (see below).

`token_usage.metadata.attribution` records `"traceparent"` on every row; since
rejection replaced the old best-effort fallback, no other value can occur and no
unattributed usage rows exist.

Direct-chat flows used to stay `running` forever; `agent_proxy` marks them completed
when the response finishes (`complete_flow` + the SSE tap), so a flow row's liveness
is meaningful.

## Strict enforcement: no flow, no tokens

The router **discards** any LLM call it cannot attribute, rather than serving it with
degraded billing. Rationale: unattributed usage is not just a FinOps gap — flow token
budgets (`FLOW_MAX_TOKENS`) and per-user accounting key off the flow, so an agent that
drops its traceparent would burn tokens outside every cascade limit. The MCP gateway
applies the same rule to `tools/call` for the same reason: user identity resolved from
the flow record is what permissions and budgets are computed from.

Mechanics:

- **Status code is `403`, never `401`.** The agent's credential is valid —
  authentication succeeded; what's missing is authorization context. A `401` would
  send well-behaved clients into credential-refresh loops.
- **Presence is not the check — resolution is.** A random well-formed traceparent
  resolves to no `flows` row and is rejected identically. There is nothing an agent
  can fabricate to pass.
- **The error body must be descriptive** (`"traceparent missing or does not resolve
  to an active flow"` + the received header value, if any). The most common cause is
  the silent Rust gotcha below — a malformed header the W3C propagator drops without
  logging — and a bare 403 there is miserable to debug.
- **Embeddings enforce the same rule** (`/v1/embeddings` previously attempted no
  attribution at all). Consequence, accepted deliberately: an agent cannot embed
  outside a user flow — startup/ingest-time indexing is not currently supported.
  If needed later it becomes an explicit owner-billed mode, not a silent fallback.
- **The `flows` row write is load-bearing on every dispatch path** (agent proxy,
  a2a dispatch, react-agent A2A tool, MAF executor). A path that forgets the write
  is an agent outage under this rule, not a billing gap — integration tests cover
  each path.

The old active-flow fallback (attribute to the agent's sole running flow when no
traceparent arrived) is **removed**, not merely disabled: a fuzzy fallback behind a
strict gate defeats the gate.

## The `traceparent` contract, per language

Forwarding only works if the agent's HTTP stack actually propagates the header:

| Language | Mechanism | Code needed |
|---|---|---|
| Python | OTel auto-instrumentation (`opentelemetry-instrument`) | none |
| Node.js | `@opentelemetry/auto-instrumentations-node` | none |
| Java / .NET | OTel javaagent / auto-instr | none |
| Go | **loongsuite `otel go build`** (compile-time auto-instr of `net/http`) in the Dockerfile | none — do **not** add `otelhttp` deps; loongsuite pins its own OTel version and conflicts |
| Rust | No auto-instrumentation exists | **manual** — see below |

### The Rust gotcha

`Span::current().context()` inside a `#[tracing::instrument]`ed `chat()` is a *fresh
local span*, not the platform-re-homed trace — injecting from it silently produces
nothing. The working pattern in every Rust agent:

1. `execute()` reads the inbound header (`ctx.service_params["traceparent"]`) and
   builds a remote `opentelemetry::Context` (`remote_context_from_traceparent`).
2. That `parent_cx` is **threaded explicitly** into `chat()` / the LLM call.
3. The outbound request injects via `telemetry::traceparent_for_context(parent_cx)`.

Also: W3C validation is strict — a malformed `traceparent` (e.g. wrong span-id length)
is silently discarded by `TraceContextPropagator`. When debugging "no attribution",
check the header bytes first.

## Why `traceparent` and not a custom header

It is the **only** header every OTel auto-instrumentation forwards for free, in every
language, with zero agent code — and it was already carrying the same identity for
distributed tracing. One header, one identity, two consumers (Tempo traces + router
attribution). A2A SDKs propagate no headers by themselves; the platform's agents
extract/inject explicitly where auto-instrumentation doesn't exist.

## Read path (TokenOps / sessions)

- Every usage row is fully attributed by construction — unattributable calls are
  rejected before any tokens are spent, so there is no NULL-user usage to reconcile.
- The finops dashboard (`agent_finops` / `agent_stats`) used to find an agent's traces
  by searching Tempo for `session.id`, which uninstrumented agents never set. It now
  unions Tempo results with the Postgres `session_traces` index (written by the proxy)
  via `SessionIdResolver::traces_for_agent`, and excludes soft-deleted agents so
  redeployed same-name agents don't double-count.
