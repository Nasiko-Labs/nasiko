# A2A Proxy Failure-Injection Findings

Results from the chaos suite. Each row maps to a scenario in the harness and
to invariants in `docs/chaos/INVARIANTS.md`.

Fill **Result** with `PASS`, `FAIL`, or `ENVIRONMENT`. Leave **Evidence** as a
path, command, or short note — never tokens or secrets.

| ID | Scenario | Invariants | Result | Evidence | Notes |
|----|----------|------------|--------|----------|-------|
| C1 | Kill agent mid-stream | I8, I11 | | | |
| C2 | Upstream latency | I9 | | | |
| C3 | Partial / truncated response | I9 | | | |
| C4 | Credential expiry or revocation mid-session | I6 | | | |
| C5 | Direct access bypassing the proxy | I1, I12 | | | |
| C6 | ACL-denied invoke | I4 | | | |
| C7 | Saturate A2A rate limits | I7 | | | |
| C8 | Header leak / identity spoof | I2, I3 | | | Baseline: `server/tests/agent_proxy_authz.rs` |
| C9 | FlowGuard cascade bounds | I10 | | | Optional |

## Vocabulary

| Result | Meaning |
|---|---|
| **PASS** | The mapped invariants held. |
| **FAIL** | An invariant was violated; describe the repro in Notes. |
| **ENVIRONMENT** | Observed on this runtime (for example Docker Desktop loopback) and must not be claimed as production VPC behaviour. |
