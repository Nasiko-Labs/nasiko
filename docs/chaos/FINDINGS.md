# A2A Proxy Failure-Injection Findings

Results from the chaos suite. Each row maps to a scenario in the harness and
to invariants in `docs/chaos/INVARIANTS.md`.

Fill **Result** with `PASS`, `FAIL`, or `ENVIRONMENT`. Leave **Evidence** as a
path, command, or short note — never tokens or secrets.

| ID | Scenario | Invariants | Result | Evidence | Notes |
|----|----------|------------|--------|----------|-------|
| C1 | Kill agent mid-stream | I8, I11 | ENVIRONMENT | `docker kill f557c072a338`; `docker ps` then showed no `nasiko-agent-*` | Container process died. The dashboard already showed a complete translation, so in-flight abort (I8) was not captured. Catalog still showed Active after the kill (I11: operator status lagged the runtime). Agent later restored with `docker start` on the same container. |
| C2 | Upstream latency | I9 | PASS | `server/tests/chaos_proxy.rs` `i9_slow_upstream_does_not_crash_proxy` | Stub sleeps 2s; proxy returns 200; `/health` stays 200. |
| C3 | Partial / truncated response | I9 | PASS | `server/tests/chaos_proxy.rs` `i9_truncated_upstream_does_not_hang_proxy` | Stub closes after a partial HTTP body; proxy returns within 15s; `/health` stays 200. |
| C4 | Credential expiry or revocation mid-session | I6 | PASS | `server/tests/chaos_proxy.rs` `i6_revoked_session_cannot_invoke_proxy` | Issued session reaches the stub (200); after `auth_tokens.revoked_at` is set, the same bearer returns 401. |
| C5 | Direct access bypassing the proxy | I1, I12 | ENVIRONMENT | Live `127.0.0.1:50078` (container `f557c072a338`, `8000/tcp -> 127.0.0.1:50078`); `server/tests/chaos_proxy.rs` `i1_unauthenticated_agent_route_returns_401` and `i12_direct_agent_listen_ignores_platform_bearer` | Developer Docker publishes the agent on loopback. `GET /.well-known/agent-card.json` returns 200 with no Nasiko session. `POST /` with a forged Bearer is unchanged vs no auth (JSON-RPC application response). Unauthenticated `:8080` does not invoke the agent (`303` to `/login.html` on the compose server; `401` in `TestServer`). Not a production VPC proof (`docs/BOOTSTRAP_AND_NETWORKING.md`: agents are not public ingress). |
| C6 | ACL-denied invoke | I4 | PASS | `server/tests/agent_proxy_authz.rs` `proxy_rejects_non_owner_non_grantee_with_404` | Direct proxy returns 404; stub agent is not invoked. |
| C7 | Saturate A2A rate limits | I7 | PASS | `server/tests/chaos_proxy.rs` `i7_a2a_dispatch_returns_429_after_burst` | 31st authenticated `POST /api/orchestrator/a2a` in 60s returns 429 with a visible body. |
| C8 | Header leak / identity spoof | I2, I3 | PASS | `server/tests/agent_proxy_authz.rs` `proxy_strips_credentials_and_spoofed_identity_headers` | Agent echo has no `Authorization`/`Cookie`; spoofed `x-user-*` is not forwarded. |
| C9 | FlowGuard cascade bounds | I10 | PASS | `flow/tests/flow_guard.rs` (`guard_check_without_redis_fails_closed`, cycle/fan-out/token cases); `server/src/agent_proxy.rs` maps `FlowRejection` to HTTP 508 (`LOOP_DETECTED`) | Depth, cycle, fan-out, token budget, and timeout rejections are enforced in Redis-backed `FlowGuard`. Unreachable Redis fails closed. Direct proxy returns 508 on cascade reject. |

## Vocabulary

| Result | Meaning |
|---|---|
| **PASS** | The mapped invariants held. |
| **FAIL** | An invariant was violated; describe the repro in Notes. |
| **ENVIRONMENT** | Observed on this runtime (for example Docker Desktop loopback) and must not be claimed as production VPC behaviour. |
