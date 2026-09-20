# A2A Proxy Invariants

Normative rules for Nasiko’s agent reverse-proxy and orchestrator dispatch.
Language is **must** / **must not**. Evidence is an HTTP status, a stub-agent
header dump, a bounded client timeout, or an operator-visible log/status.

These invariants are the contract for the chaos and failure-injection suite.
Tests and live scripts map to the IDs below.

## Catalogue

| ID | Name | Statement | Primary evidence |
|---|---|---|---|
| **I1** | Sole authenticated ingress | Requests to `/api/agents/{id}` without a valid user session **must** fail with **401**. | HTTP status |
| **I2** | No platform credential leak | The agent **must not** receive `Authorization` or `Cookie` from the caller. | Stub echo of headers |
| **I3** | No identity spoof | Client-supplied `x-user-id` / `x-username` / `x-is-superuser` **must not** be forwarded. Server-injected `x-user-id` **must** equal JWT `sub`. | Stub echo |
| **I4** | Invoke ACL fail-closed | A caller who cannot access a private agent **must not** cause work on that agent. Direct proxy **must** respond **404** (existence not confirmed). Orchestrator may respond **403**; both codes **must** be recorded, not conflated. | HTTP status; stub not invoked |
| **I5** | Token audience | An **agent** JWT **must not** authenticate the **user** proxy path. | 401 |
| **I6** | Session liveness | After expiry or revocation, further proxy calls **must** be **401**. An open SSE **must not** indefinitely continue to authorize as that user unless that policy is documented. | HTTP / stream end |
| **I7** | Visible throttling | Excess A2A dispatch **must** return **429** with a stable, non-empty error body. | HTTP 429 |
| **I8** | Terminal upstream death | If the agent process dies during an in-flight stream, the client **must** observe stream termination or a proxy error within the HTTP client timeout. The server process **must not** hang indefinitely. | Time-bounded client |
| **I9** | Bounded slow/partial upstream | Sleeping or truncated agent responses **must not** crash `nasiko-server`. The client **must** receive timeout, 502, incomplete SSE then close, or an equivalent documented error. | Process health + client |
| **I10** | Cascade bounds | Depth, cycle, fan-out, token budget, and flow timeout violations **must** reject (508 or documented JSON-RPC). Redis unavailability **must** fail closed. | HTTP / JSON-RPC |
| **I11** | Observable failure | After agent death, operators **must** be able to see status, logs, or proxy errors via API or dashboard. Failures **must not** be silent. | API / UI / server logs |
| **I12** | Bypass posture | Direct connections to the agent listen address **must not** confer platform authority. Reachability from untrusted networks **must** match the deployment document. Loopback publish on developer Docker is classified as **environment**, not as a production VPC proof. | Direct `host:port` vs `:8080` |

## Result vocabulary

| Result | Meaning |
|---|---|
| **PASS** | The invariant held under the injection. |
| **FAIL** | The invariant was violated; include a repro. |
| **ENVIRONMENT** | True in this runtime (for example Docker Desktop loopback) and must not be over-claimed for VPC production. |

## Related implementation

| Concern | Location |
|---|---|
| Direct proxy | `server/src/agent_proxy.rs` |
| Orchestrator dispatch | `server/src/router/a2a_dispatch.rs` |
| ACL | `server/src/acl.rs` |
| Rate limiting | `server/src/rate_limit.rs` |
| FlowGuard | `flow/src/guard.rs` |
| Baseline header/ACL tests | `server/tests/agent_proxy_authz.rs` |
