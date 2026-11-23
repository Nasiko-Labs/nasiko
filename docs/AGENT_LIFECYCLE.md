# Agent Development Lifecycle

Build, test, deploy, and operate A2A agents on Nasiko — from `nasiko new` to production.

## Overview

The lifecycle has four phases:

**Create** → **Test** → **Deploy** → **Operate**

You don't need a cluster to start developing. Create and test your agent locally — a cluster is only needed when you're ready to deploy.

## Prerequisites

- Docker running locally
- `nasiko` CLI installed

## Project Structure

An agent project has two required files and one auto-generated file:

    my-agent/
      AgentCard.json          ← you manage this (or `nasiko card` generates it)
      Dockerfile              ← you manage this (or `nasiko new` scaffolds it)
      src/                    ← your agent source code
      .nasiko/agent.json      ← auto-created by `nasiko deploy` (do not edit)

**AgentCard.json** is the single source of truth for your agent's identity. It contains:

- **name** — unique identifier on the cluster
- **version** — image tag used for deploys (e.g. "0.1.0")
- **description** — what the agent does
- **skills** — capabilities the agent exposes
- **capabilities** — streaming, push notifications, etc.
- **url** — agent endpoint
- **protocolVersion** — A2A protocol version (e.g. "0.2.9")
- **preferredTransport** — JSONRPC

Which commands use which files:

- `nasiko card` — generates **AgentCard.json** (from source + description)
- `nasiko validate` — checks both **AgentCard.json** and **Dockerfile** exist and are valid
- `nasiko build` — reads **Dockerfile** to build; reads **AgentCard.json** for the image tag (name:version)
- `nasiko run` — same as build, then starts the container
- `nasiko deploy` — reads **AgentCard.json** for name + version, builds, pushes, deploys; writes **.nasiko/agent.json** to track the agent ID on the cluster

The `.nasiko/agent.json` file links your local directory to a deployed agent — so subsequent `nasiko deploy` updates the same agent instead of creating a new one. Add it to `.gitignore`.

---

## Phase 1: Create

### Scaffold a new agent

    nasiko new                      # interactive — pick a template
    nasiko new openai my-agent      # from template directly
    nasiko new claude-sdk my-agent  # Claude SDK template

Templates are pulled from the artifact registry (Nasiko's public registry is connected by default). This creates a project directory with:

- **AgentCard.json** — A2A agent metadata (name, version, skills, endpoint)
- **Dockerfile** — container build instructions
- **Source code** — A2A protocol handler

### Generate or update the agent card

If you're connected to a cluster, the card is generated using an LLM (reads your source code + description). Otherwise it falls back to interactive prompts.

    nasiko card "A code review agent that finds bugs in PRs"
    nasiko card                     # omit description to auto-detect from source
    nasiko card --dir ./my-agent    # specify directory

### Validate your agent project

    nasiko validate                 # check structure, AgentCard.json, Dockerfile

---

## Phase 2: Test

### Build the container image

    nasiko build                    # build from current directory
    nasiko build --tag my-agent:v2  # custom tag
    nasiko build ./path/to/agent    # specify directory

### Run locally

    nasiko run                      # build + start on port 8000
    nasiko run --port 8080          # custom port

Your agent is now running at http://localhost:8000. It speaks the A2A protocol — no cluster needed.

### Chat with your agent

Interactive mode:

    nasiko chat http://localhost:8000

One-shot:

    nasiko chat http://localhost:8000 "What can you do?"

Full-screen TUI:

    nasiko chat http://localhost:8000 --tui

Resume a previous session:

    nasiko sessions
    nasiko chat http://localhost:8000 --resume <session-id>

---

## Phase 3: Deploy

When you're ready to deploy, connect to a cluster (remote or local).

### Connect to a cluster

Remote cluster:

    nasiko connect https://nasiko.example.com --name prod
    nasiko auth login

Or start a local cluster from a clone of the repository (auto-registers as "local"):

    git clone https://github.com/Nasiko-Labs/nasiko.git && cd nasiko
    nasiko up
    nasiko auth login     # admin / changeme, or the ADMIN_PASSWORD in .env

`nasiko up` runs the README's Docker Compose quick start for you: in the checkout it creates `.env` from `.env.example` if there is none, runs `docker compose up -d --build` (Podman works too), waits up to two minutes for the server's `/health`, and registers `http://localhost:8080` as a cluster named "local" — visible in `nasiko clusters` alongside any remote ones. It must be run inside the clone (any directory within it works); the first build takes a few minutes.

    nasiko clusters       # see all configured clusters
    nasiko use prod       # switch active cluster
    nasiko down           # stop the local stack (run in the clone; data is kept)

### Deploy

    nasiko deploy .                          # build, push, deploy from current dir
    nasiko deploy . --name my-agent          # explicit name
    nasiko deploy . --port 8080              # non-default container port
    nasiko deploy . --env-file .env          # inject secrets from file
    nasiko deploy . -e API_KEY=sk-xxx       # inline env vars

This builds the Docker image, pushes it to the cluster's OCI registry, and starts the container. Agent names are unique per cluster — deploying the same name again updates the existing agent.

### Push without deploying

Useful for CI or when you want to separate image upload from deployment:

    nasiko push .
    nasiko push . --name my-agent

### Deploy from the registry

Deploy an agent that is already published to an artifact registry, without
building anything locally — the control plane pulls it itself, so no Docker
daemon is needed on your machine:

    nasiko import nasiko/my-agent:1.0.0                       # resolved against the connected registry
    nasiko import registry.example.com/nasiko/my-agent:1.0.0  # or fully qualified

The short `owner/name[:tag]` form resolves against whatever `nasiko registry
connect` points at, the same way `nasiko new` resolves a template. The tag
defaults to `latest`.

Both packaging formats are accepted: a container image is pulled and deployed
directly, while a source artifact is downloaded, built server-side, and then
deployed (the command prints a `build_id` in that case — follow it with
`nasiko logs <agent>`).

The registry host must be listed in the control plane's
`REGISTRY_IMPORT_ALLOWED_HOSTS`, otherwise the import is refused. Registries
that require authentication are not supported yet — the server fetches
manifests anonymously.

### Manage secrets

Secrets are encrypted env vars injected into agent containers at runtime. There are two scopes:

**Vault secrets** (the UI's Settings → Secrets) — yours, applied to every agent you own:

    nasiko secrets set OPENAI_API_KEY sk-xxx
    nasiko secrets ls
    nasiko secrets rm OPENAI_API_KEY

**Agent-specific secrets** (the agent's Settings tab → Secrets) — override vault, applied to one agent only:

    nasiko secrets set DB_URL postgres://... --agent my-agent
    nasiko secrets ls --agent my-agent
    nasiko secrets rm DB_URL --agent my-agent

**Precedence** (highest wins): inline deploy `-e` > agent secrets > vault secrets. An agent gets its
**owner's** vault secrets, whoever deploys it. Every deploy path (upload, update, rollback, restart,
startup recovery, a parked agent's wake) builds the env the same way: vault, then agent secrets on
top (`resolve_secret_env` in `server/src/catalog/agent_secrets.rs`); platform defaults fill only
names neither sets, and the platform's own keys (see [Platform environment](#platform-environment))
win over all of them.

The agent's Secrets section lists the vault names it inherits (names only, never values), marked
**Overridden here** where an agent secret of the same name wins
(`GET /api/agents/{id}/secrets/inherited`).

A secret is read when the agent starts, so a change to either scope reaches a running agent only
after `nasiko restart` (or Restart in the UI) or a redeploy; a parked agent picks it up on its next
wake.

---

## Phase 4: Operate

### List running agents

    nasiko ps                       # table view
    nasiko ps --json                # machine-readable

### View logs

    nasiko logs my-agent            # last 50 lines
    nasiko logs my-agent -n 200    # more history

### Lifecycle management

    nasiko stop my-agent            # stop (scale to 0, keeps state)
    nasiko start my-agent           # resume a stopped agent
    nasiko restart my-agent         # redeploy with latest secrets/env
    nasiko rm my-agent              # terminate and deregister
    nasiko rm my-agent -f           # force removal without confirmation

**restart** picks up any secrets you've changed since the last deploy. On a runtime that can replace a running agent in place it does so one instance at a time, so the agent stays available and keeps its replica bounds; otherwise (the agent is stopped, a fresh image pull was requested, or the runtime cannot) it destroys and recreates the container. **stop/start** just pause and resume without changing configuration.

### Scaling

Every agent runs one replica; `nasiko scale <agent> <n>` sets a fixed count. An agent that may be
parked (see [Parked agents](#parked-agents)) runs exactly one: scaling it above one is refused with
`409` and code `parkable`; set its parking policy to `resident` first. Per-agent autoscaling is an
enterprise feature.

### Parked agents

A parked agent's workload is scaled to zero. Its image, registry entry and workload stay in place,
its status reads `parked`, and the next request starts the same workload again and waits until it
answers, so the agent keeps its env, ports and mounts (a `--writable` agent finds its files again).

Every deploy starts the agent's workload. When an agent that may be parked has had no request,
and no deploy or other change, for `AGENT_PARK_IDLE_SECS` (default `3600`), has nothing in
flight, has made no model call in that time, and has no open task or pending human-in-the-loop
question younger than two hours, the platform scales its workload to zero and its status reads
`parked`. A task the agent held in memory is lost when it is parked.
`AGENT_PARK_IDLE_SECS` must be at least `AGENT_CALL_TIMEOUT_SECS` plus 60 seconds (how often at
most a request records the agent's last-request time) plus one `SESSION_REAPER_INTERVAL_SECS`, or
the server refuses to start; left unset it is raised to that. A request that reaches the agent just
as it is parked wakes it again, or is answered `503` with `Retry-After` to retry.
`AGENT_PARK_IDLE_SECS=0` turns agent parking off; agents already parked still start on their next
request. It is independent of `CODING_AGENT_IDLE_TTL_SECS`, which governs harness sessions only.

An agent may be parked when it is an ordinary agent (not a connector or harness) and its
parking policy is `auto` (the default for a new agent). Nothing else decides it. Agents that
existed when this release was installed were set to `resident`, so they keep running as before;
agents already parked keep `auto`.

To keep an agent always running, set its policy to `resident` with the `parking_policy`
field (`auto` or `resident`) of the agent metadata update, `PUT /api/agents/{id}`. A `parked`
agent set to `resident` is woken by its next request and then stays up. A running `resident`
agent set to `auto` is parked once it is idle. The upload (`POST /api/agents/upload`) and
multipart update (`PUT /api/agents/{id}/update`) forms and the GitHub clone request
(`POST /api/github/clone`) take the same optional field, applied to that deploy; absent keeps the
stored policy (`auto` for a new agent). The multipart update saves the policy as it accepts the
request, so an update whose build then fails keeps the new policy. Platform agents (`SEED_AGENTS`
and internal agents) are always `resident`. The CLI has no flag for it yet.

`agents.sandbox_compatible` records whether the agent's image is known to start under gVisor
(`runsc`): `true` it did, `false` it did not, empty it was never checked. Nothing reads it to
refuse a deploy; it is information for operators. To fill it in, set `BUILD_SANDBOX_SMOKE=runsc`
(default `off`) on a Docker-runtime server with gVisor installed: after each successful build the
worker runs the image once under `--runtime=runsc`, waits up to 60 seconds for its port to accept
connections, and stops it. A host without gVisor leaves the column empty (logged once), any other
failure sets `false` with the reason in the build log, and the build itself never fails because of
the smoke run. On Kubernetes the check is skipped, and agents do not run under gVisor there.

The lifecycle commands work on a parked agent too: `nasiko stop` leaves it `stopped` (a request no
longer wakes it), and `nasiko start` wakes it the way its next request would, answering once the
agent serves (`503` with the reason if it does not start in time, and it stays `parked`). An agent
parked by an earlier release has no workload; its next request (or `nasiko start`) deploys one from
its stored image.

In the web UI a parked agent's page says what parked means and offers Start and Stop beside Try it,
and the agent's Settings tab has an Always on switch: on sets the policy to `resident`, off to
`auto`. Where every idle agent is parked whatever its policy, the switch is shown off and disabled.

`nasiko ps` reads the agent list, so a parked agent appears in it with state `parked`; only
`GET /api/containers` lists workloads.

### Cluster health

    nasiko status                   # control plane health + metrics

### Observability & usage attribution

The control plane injects the standard `OTEL_*` environment variables into every
deployment (collector endpoint, `OTEL_SERVICE_NAME`, GenAI capture mode) and routes your
agent's LLM calls through its LLM gateway (`OPENAI_BASE_URL` etc.).

**Forwarding `traceparent` is mandatory, not best-effort.** Every LLM call (and every
MCP `tools/call`) must carry the W3C `traceparent` your agent received on its inbound
A2A request — the platform resolves the calling user and the flow's budgets from it,
and **rejects unattributable calls with `403`** rather than serving them with degraded
billing. There is no fallback attribution.

| Your agent | LLM / MCP calls served | Distributed traces | Per-user/session attribution |
|---|---|---|---|
| Forwards `traceparent` | ✅ | ✅ | ✅ precise (the trace id names the flow) |
| Drops `traceparent` | ❌ `403` on every call | ❌ | — |

How to forward it, per language — all driven by the injected `OTEL_*` env vars:

| Language | How |
|---|---|
| Python | `opentelemetry-instrument python main.py`, or initialize the SDK yourself (see `agents/common/telemetry.py`) |
| Node.js | Set `NODE_OPTIONS="--require @opentelemetry/auto-instrumentations-node/register"` in your Dockerfile, with the package in your image |
| Java | OTel javaagent: `JAVA_TOOL_OPTIONS="-javaagent:/path/to/opentelemetry-javaagent.jar"` |
| Go | loongsuite `otel go build` in the Dockerfile (compile-time `net/http` instrumentation), or copy the manual-forwarding fallback in `agents/weather` |
| Rust | No auto-instrumentation exists — initialize the OTel SDK in code and inject the header explicitly (see the Rust agents' `telemetry.rs`) |

Minimum: extract `traceparent` from your **inbound HTTP server** and inject it on your
**outbound LLM/MCP clients**. Token counts themselves come from the LLM provider's
response — no GenAI-specific instrumentors are required.

### Tools via the MCP gateway

Every deployment also receives the platform's tool layer, configured entirely by env:

- `MCP_GATEWAY_URL` — the gateway's single JSON-RPC endpoint (when the platform has one configured)
- `MCP_GATEWAY_TOKEN` — your agent's own credential, minted fresh on every deploy

Configure your MCP client **once at startup** — there is no per-request credential:

```python
mcp = McpClient(
    url=os.environ["MCP_GATEWAY_URL"],
    headers={"Authorization": f"Bearer {os.environ['MCP_GATEWAY_TOKEN']}"},
)
```

`tools/list` works immediately (agent-only identity — use it for startup discovery).
`tools/call` is authorized per flow: the gateway resolves the calling user from your
forwarded `traceparent` and requires that the flow was actually dispatched to your
agent, so the same OTel propagation above is all you need. The Go reference
implementation is `agents/weather` (`mcp.go`).

### Durable state

Any A2A agent runs on the platform, whatever it does with its state. An agent that keeps tasks
in process memory, as the SDK's `InMemoryTaskStore` does, loses paused work on every restart and
has to stay at one replica. Keeping tasks in the platform's state store is necessary for
surviving restarts and running more than one replica, but not sufficient: every other piece of
state the agent keeps between requests must be durable too. For example `agents/crewai` keeps
data in its own `InMemoryCache`, so it is not replica-safe even though it uses the durable task
store. Google ADK sessions and JavaScript task stores are covered below.

The platform injects the store's address and your credential into every deployment:

| Variable | Meaning |
|---|---|
| `NASIKO_STATE_URL` | Base URL of the state API (it ends in `/api/agent-state`). Unset means the platform provides no store. |
| `NASIKO_AGENT_TOKEN` | Your agent's state credential, sent as a `Bearer` token. Required when `NASIKO_STATE_URL` is set; the MCP gateway token is a different credential and is not accepted. |

**Python (`a2a-sdk` 0.3.x and 1.x).** An image is built from your agent's directory only, so copy
`agents/common/nasiko_state.py` next to your `main.py`, change one constructor argument and
hook the store into the server's shutdown:

```python
from nasiko_state import install_shutdown_hooks, make_task_store

handler = DefaultRequestHandler(
    agent_executor=MyExecutor(),
    task_store=make_task_store(),  # was InMemoryTaskStore()
    agent_card=card,
)
app = Starlette(routes=routes)
install_shutdown_hooks(app)  # flushes pending writes when the server stops
```

`make_task_store()` returns the SDK's in-memory store when `NASIKO_STATE_URL` is unset, so the
same code runs unchanged on your laptop; otherwise it registers your agent as durable at startup,
in the background. A LangGraph agent swaps `MemorySaver()` for `make_checkpointer()` the same
way. `agents/common/check_vendored.py` fails CI when a copy drifts from the original. Each
agent may make 1200 state calls a minute per ready replica by default
(`AGENT_STATE_RATE_LIMIT_PER_MINUTE`, so the budget grows as the agent scales); a rate-limited call
waits out `Retry-After` (30 s at most), then raises. A turn of `agents/langgraph` (its graph
runs with `durability="exit"`: one checkpoint per run) costs 7 calls, 8 on a2a-sdk 1.x.

**What is written, and when.** The store never pretends a write happened:

- A save that changes `task.status.state` (a task's first save, `input-required`, every terminal
  state) is on the server before `save()` returns.
- Any other save, such as streamed artifact chunks, is coalesced to at most one write per 500 ms
  per task. A pending write goes out when its window ends, when the task changes state, and when
  the server shuts down through `install_shutdown_hooks` (uvicorn shuts down on SIGTERM).
- After a hard crash (SIGKILL, power loss) at most the last 500 ms of those non-status updates
  is lost. A state change that `save()` returned for always survives.

Failures raise `NasikoStateError` with the HTTP status and body; there is no silent fallback to
memory once `NASIKO_STATE_URL` is set. Every task write is conditional on the version the replica
last saw, so a replica never overwrites a newer write: a state change that loses raises
`NasikoStateConflict`, and a coalesced update that loses (or waited over 60 s) is dropped. Design
around two limits: a task, or one LangGraph channel value, above the platform's body limit (1 MiB
by default) raises `NasikoStateTooLarge`, which is why `agents/doc-reader` keeps its
document-carrying tasks in memory; and stored state expires after the platform's retention
window (7 days by default) counted from its last write.
### Platform environment

Every deploy path (upload, update, rollback, restart, import, seed, startup recovery) assembles
the same platform-injected keys, in one place (`server/src/agents/deploy_env.rs`). They are
set after your own secrets and `env` field, so a value you supply under one of these names is
replaced, never used.

| Name | Meaning |
| --- | --- |
| `MCP_GATEWAY_URL` | The MCP gateway's JSON-RPC endpoint (set when the platform has one configured) |
| `MCP_GATEWAY_TOKEN` | Your agent's credential for the gateway; rotated on every deploy |
| `MCP_GATEWAY_CONNECT_URL` | The gateway endpoint with the credential in the path, for MCP clients that accept only a URL |
| `OPENAI_BASE_URL`, `OPENAI_API_KEY` (or the Anthropic/Gemini equivalents) | Routes your LLM SDK through the platform's LLM gateway with an agent identity token |
| `NASIKO_STATE_URL` | Base URL of the agent state API (`/api/agent-state`). Absent when `MCP_GATEWAY_PUBLIC_URL` does not end in `/api/mcp` |
| `NASIKO_AGENT_TOKEN` | Your agent's credential for the state API, separate from the gateway credential; rotated on every deploy. Absent together with `NASIKO_STATE_URL` |

A rotated credential keeps its predecessor valid for 15 minutes, so a rollout still in progress
is not cut off. A deploy of a new image (update, rollback, upload, import) marks the agent's state
as in-memory again until your task store registers at startup; a restart or startup recovery of the
same image leaves the recorded mode as it was.

**Google ADK sessions.** ADK keeps conversations in a session service, and the stock
`InMemorySessionService` loses them on restart. Copy `agents/common/nasiko_adk_sessions.py`
(it imports `nasiko_state.py`, so copy that too) and swap the service:

```python
from nasiko_adk_sessions import session_service_from_env

runner = Runner(
    agent=agent,
    app_name=APP_NAME,
    session_service=session_service_from_env(),  # was InMemorySessionService()
)
```

Sessions, and the `user:` and `app:` state they share, are stored as key-value rows under
`adk/...` keys; an append that loses a race with another replica is re-read and re-applied, and
fails with `NasikoStateError` after a few attempts instead of dropping the event. Because a
durable session outlives the process, create it only when `get_session` finds none (ADK refuses
to create one that exists), as `agents/google-adk` does. This registers key-value state only:
it does not make the agent's tasks durable, so pair it with `make_task_store()`.

**JavaScript (`@a2a-js/sdk` 1.x).** `agents/common/js/nasiko-state` is a dependency-free ESM
package (plain JavaScript with JSDoc types, so it needs no build step and can be copied into an
image like the Python modules). It provides `NasikoTaskStore`, the same write rules and
version-conditional writes as the Python store, and stores tasks in a format the Python store
reads too:

```js
import { DefaultRequestHandler } from "@a2a-js/sdk/server";
import { installShutdownHooks, taskStoreFromEnv } from "nasiko-state";

const handler = new DefaultRequestHandler(agentCard, taskStoreFromEnv(), executor);
installShutdownHooks(); // flush pending writes on SIGTERM and SIGINT
```

`taskStoreFromEnv()` returns the SDK's `InMemoryTaskStore` when `NASIKO_STATE_URL` is unset. The
package also exports `NasikoStateClient` for your own key-value state (`putKv`, `getKv`,
`listKv`, ...), with typed errors for 401, 412, 413 and 429. The 0.3.x JavaScript SDK is not
supported: a row written in another format is refused with `NasikoStateMalformedRow`.

---

## Phase 5: Share (Registry)

### Connect to the artifact registry

A public registry is connected by default. You can also use a private one:

    nasiko registry connect https://registry.example.com
    nasiko registry disconnect
    nasiko registry status

### Browse and search

    nasiko registry search "code review"     # search by keyword
    nasiko registry search -t agent          # filter to agents
    nasiko registry list                     # list everything
    nasiko registry list --json              # machine-readable

### Publish

Publishing to a shared artifact registry is available in the Nasiko enterprise edition.

---

## Typical Workflow

    # Scaffold
    nasiko new openai code-reviewer
    cd code-reviewer

    # Test locally (no cluster needed)
    nasiko validate
    nasiko run
    nasiko chat http://localhost:8000 "Review this PR: ..."

    # Deploy
    nasiko connect https://nasiko.example.com --name prod
    nasiko auth login
    nasiko deploy .

    # Monitor
    nasiko ps
    nasiko logs code-reviewer

---

## Iterating on a deployed agent

Code changes — redeploy (rebuilds image, pushes, restarts container):

    nasiko deploy .

Changed secrets only — restart to pick them up (no rebuild):

    nasiko secrets set NEW_KEY value --agent my-agent
    nasiko restart my-agent

Published a new version to the registry — re-run the import with the new tag:

    nasiko import nasiko/my-agent:1.0.1

This updates the agent in place rather than creating a second one: it matches
on owner + name, so the agent keeps its id, and its grants, ACLs, and stored
secrets survive. The control plane re-pulls the image and recreates the
container.

Prefer bumping the tag. Re-publishing the *same* tag also works — the pull
compares digests and the container is recreated either way — but the recorded
version no longer identifies which build is running.

Note that `nasiko restart` does **not** fetch a new image; it recreates the
container from what the cluster already has. Only `deploy` and `import` bring
in new code.

---

## Local cluster workflow

Test the full deploy flow locally before pushing to production:

    # in your clone of https://github.com/Nasiko-Labs/nasiko
    nasiko up                       # build and start the Compose stack, register it as "local"
    nasiko auth login               # admin / changeme, or the ADMIN_PASSWORD in .env

    # in your agent's directory
    nasiko deploy .                 # deploys to local
    nasiko ps                       # verify it's running
    nasiko chat my-agent "test"     # name resolves to the agent's UUID

    # back in the clone
    nasiko down                     # stop the stack; data stays in its volumes

---

## CI/CD Integration

    # Build and push (CI job)
    nasiko connect $CLUSTER_URL
    nasiko auth login               # interactive: prompts for username and password
    nasiko push . --name my-agent

    # Deploy (CD job or manual gate)
    nasiko deploy my-agent:latest

`nasiko auth login` has no non-interactive form yet: it reads the password from a terminal
(`--password` only skips SSO, it does not take a value). In CI, sign in once on a machine with a
terminal and make that session available to the job, or run the job with a TTY.

---

## Command Reference

**Create**

- `nasiko new [template] [name]` — Scaffold agent project
- `nasiko card [description]` — Generate AgentCard.json
- `nasiko validate` — Validate project structure

**Test**

- `nasiko build` — Build Docker image
- `nasiko run` — Run agent locally
- `nasiko chat <url>` — Chat via A2A protocol

**Deploy**

- `nasiko connect <url>` — Register cluster
- `nasiko up` / `nasiko down` — Start/stop local cluster
- `nasiko deploy .` — Build + push + deploy
- `nasiko push .` — Push image only
- `nasiko import <ref>` — Deploy from a registry reference (server pulls it)
- `nasiko secrets set <key> <value>` — Store secret (vault or `--agent`)

**Operate**

- `nasiko ps` — List running agents
- `nasiko logs <agent>` — Stream logs
- `nasiko stop <agent>` — Stop (scale to 0)
- `nasiko start <agent>` — Resume stopped agent
- `nasiko restart <agent>` — Redeploy with fresh env
- `nasiko rm <agent>` — Terminate and deregister

**Share**

- `nasiko registry search` — Browse registry
- `nasiko registry list` — List all artifacts

Publishing to a shared artifact registry is available in the Nasiko enterprise edition.
