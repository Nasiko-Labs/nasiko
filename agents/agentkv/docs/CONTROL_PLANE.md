# Controller milestone

This first product slice implements durable workflow ingestion, authenticated tenant boundaries, bounded Jev decisions, ranked retention proposals and operational metrics. It does **not** yet apply retention to vLLM. Every result says `engine_applied: false`; `/health` and `/metrics` report the disconnected engine. No cache hint endpoint is exposed until its engine implementation is verified.

## Run

Use Python 3.12 and `uv sync --frozen --extra infra`. Configure `AGENTKV_AUTH_TOKENS` through your secret manager as a JSON mapping from high-entropy bearer tokens to trusted owner IDs. Tokens must have at least 32 characters. Set `AGENTKV_DATABASE` to a persistent SQLite path and optionally provide `TYPESAFE_API_KEY`. Run `uv run python -m agentkv`. It binds to localhost:8081, uses one worker and caps request concurrency at 32. SQLite holds event evidence; keep it on the private control-plane host. This milestone deliberately does not implement multi-replica coordination.

`POST /events` accepts `event_id`, `flow_id`, `revision`, `kind`, `agent`, bounded `evidence` and at most three `candidates`. The server derives the owner exclusively from its bearer-token map. Events start at revision 1 and advance consecutively. Retries reuse the identical event ID and body; they cannot renew a lease. Terminal flows cannot reopen. A 15-second lease expires if telemetry stops.

Candidates contain `agent`, `prefix_id`, `incremental_bytes`, `avoided_recompute_ms` and `known_next`. These values must ultimately come from the trusted Nasiko/engine bridge. They are **not yet measured or populated by an engine adapter**. The API accepts them solely from its authenticated control-plane clients. Shared prefixes must be represented once. The prefix helper hashes exact token IDs and all compatibility revisions using an owner-scoped keyed digest; it does not reorder or remove prompt content.

`POST /flows/{flow_id}/decide` returns deterministic decisions for known successors, or one Jev request for the uncertain candidates. Estimates refer to invocation within the next two agent transitions. There is no calibration claim. The provider has no retries and a 1.5-second deadline. On failure only known-successor proposals survive. Returned proposals expire with the source flow snapshot. A response received after the flow advances is saved as stale with no proposals. Repeating the request within this controller process reuses the saved decision. A crash between a billed provider response and its SQLite commit can still result in a repeated charge after restart; there is no claim of cross-system exactly-once execution.

`GET /flows/{flow_id}/decisions` returns the owner's historical decision records. `GET /metrics` returns owner-scoped accepted-event, active-flow, decision, fallback and stale-response counts. These are **controller metrics**, not GPU cache metrics. Historical proposals are audit records and must never be replayed as fresh engine actions.

## Cloud verification commands

- `uv run modal run infra/decision_smoke.py`: one real Jev decision, private Modal secret, bounded CPU function.
- `uv run python infra/verify_vm.py`: disposable CPU VM, Docker startup and container launch, 180-second hard timeout.
- `uv run python infra/verify_nasiko.py`: disposable CPU VM, unmodified Nasiko source pinned to commit `58cfe600559c67d58100ec2856d7b29838e2859f`, private backing services, health and authenticated API checks, 1500-second hard timeout.

Every probe explicitly terminates. The Nasiko probe generates temporary credentials inside the VM and exposes no public port. It builds the upstream Dockerfile without modifying its source. Backing-service tags are resolved at execution and image IDs captured; they are not yet a frozen benchmark environment. The published image was unsuitable for Modal because it had no amd64 manifest.

## Next integration gate

Connect actual Nasiko flow events and stable owner/model identities; populate prefix measurements from a pinned vLLM engine; implement acknowledged retention of complete unreferenced hybrid state groups. Only then run A–E cache-policy benchmarks. The Pareto module currently computes observed non-dominated points and refuses mixed cohorts, cost bases, unfinished work and failed quality thresholds. It has no measured performance dataset or uncertainty estimator yet.

## Nasiko lifecycle bridge

Set `AGENTKV_NASIKO_URL`, `AGENTKV_NASIKO_TOKEN` and `AGENTKV_NASIKO_OWNER` to enable `POST /flows/{flow_id}/sync`. The owner must be the Nasiko user UUID authenticated by that token and match the controller principal. The bridge refuses redirects and owner mismatches, reads the owner-scoped flow API, and records changes without refreshing unchanged snapshots. It does not create or execute flows. Polling can miss intermediate transitions, so this path is for lifecycle visibility, not the benchmark event feed. No cache candidate measurements are inferred from these records.
