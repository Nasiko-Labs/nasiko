# A2A Proxy Failure-Injection Findings

Results from the chaos suite. Each row maps to a scenario in the harness and
to invariants in `docs/chaos/INVARIANTS.md`.

Fill **Result** with `PASS`, `FAIL`, or `ENVIRONMENT`. Leave **Evidence** as a
path, command, or short note — never tokens or secrets.

| ID | Scenario | Invariants | Result | Evidence | Notes |
|----|----------|------------|--------|----------|-------|
| C1 | Kill agent mid-stream | I8, I11 | | | |
| C2 | Upstream latency | I9 | PASS | `server/tests/chaos_proxy.rs` `i9_slow_upstream_does_not_crash_proxy` | Stub sleeps 2s; proxy returns 200; `/health` stays 200. |
| C3 | Partial / truncated response | I9 | PASS | `server/tests/chaos_proxy.rs` `i9_truncated_upstream_does_not_hang_proxy` | Stub closes after a partial HTTP body; proxy returns within 15s; `/health` stays 200. |
| C4 | Credential expiry or revocation mid-session | I6 | PASS | `server/tests/chaos_proxy.rs` `i6_revoked_session_cannot_invoke_proxy` | Issued session reaches the stub (200); after `auth_tokens.revoked_at` is set, the same bearer returns 401. |
| C5 | Direct access bypassing the proxy | I1, I12 | | | |
| C6 | ACL-denied invoke | I4 | PASS | `server/tests/agent_proxy_authz.rs` `proxy_rejects_non_owner_non_grantee_with_404` | Direct proxy returns 404; stub agent is not invoked. |
| C7 | Saturate A2A rate limits | I7 | PASS | `server/tests/chaos_proxy.rs` `i7_a2a_dispatch_returns_429_after_burst` | 31st authenticated `POST /api/orchestrator/a2a` in 60s returns 429 with a visible body. |
| C8 | Header leak / identity spoof | I2, I3 | PASS | `server/tests/agent_proxy_authz.rs` `proxy_strips_credentials_and_spoofed_identity_headers` | Agent echo has no `Authorization`/`Cookie`; spoofed `x-user-*` is not forwarded. |
| C9 | FlowGuard cascade bounds | I10 | | | Optional |

## Vocabulary

| Result | Meaning |
|---|---|
| **PASS** | The mapped invariants held. |
| **FAIL** | An invariant was violated; describe the repro in Notes. |
| **ENVIRONMENT** | Observed on this runtime (for example Docker Desktop loopback) and must not be claimed as production VPC behaviour. |
