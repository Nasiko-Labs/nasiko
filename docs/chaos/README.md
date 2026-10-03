# A2A proxy chaos suite

How to replay the failure-injection harness. Invariants are in
`docs/chaos/INVARIANTS.md`. Results go in `docs/chaos/FINDINGS.md`.

Do not commit session tokens, `.env` values, or `SECRETS_ENCRYPTION_KEY`.

## Layer A — deterministic (`TestServer` + stub agent)

Needs the same local infra as other `nasiko-server` integration tests:
Postgres on `:5432`, Redis, and S3. On Windows, stop a host Postgres that is
occupying `:5432` so Compose can bind it, then:

```powershell
$env:TEST_PG_URL = "postgres://nasiko:nasiko@127.0.0.1:5432/nasiko_dev"
cargo test -p nasiko-server --test chaos_proxy -- --test-threads=1
```

Header isolation and ACL 404 are not duplicated here; they live in
`server/tests/agent_proxy_authz.rs`. Agent JWTs on the user proxy (I5) are
covered by `server/tests/auth_flow.rs`. FlowGuard fail-closed and cascade
limits are covered by `flow/tests/flow_guard.rs`.

The A2A limiter is process-local (`DashMap`). Each replica would allow its
own 30 requests / 60s; that is a design limit, not a harness bug.

## Layer B — live compose stack

Control plane: `http://127.0.0.1:8080`. Agent containers publish on
loopback (`127.0.0.1:<host-port>->8000`). Scripts:

| Script | Scenario |
|---|---|
| `chaos/live_c1_kill.ps1` | C1 — `docker kill` while a dashboard stream is open |
| `chaos/live_c5_bypass.ps1` | C5 — agent `host:port` vs `:8080` |
| `chaos/live_c7_ratelimit.ps1` | C7 — burst `POST /api/orchestrator/a2a` (needs `NASIKO_TOKEN`) |

`live_c1_kill.ps1` does not open the chat for you. Start a long Translator
turn in the dashboard, then run the script so the kill lands while tokens
are still arriving. Restart the agent from the Nasiko UI when possible, not
only `docker start`.

Classify loopback reachability as **ENVIRONMENT**. Do not claim VPC isolation
from a Docker Desktop publish (see `docs/BOOTSTRAP_AND_NETWORKING.md`).

## Result vocabulary

| Result | Meaning |
|---|---|
| **PASS** | The mapped invariants held. |
| **FAIL** | An invariant was violated; include a repro. |
| **ENVIRONMENT** | True on this runtime; must not be over-claimed for production. |
