# Nasiko

**The open-source control plane for AI agents.** Deploy any [A2A](https://github.com/a2aproject/a2a-spec)-speaking
agent with one command and get routing, tool access, secrets, and observability for free — no
gateway, no sidecar, no glue code.

## Why Nasiko

Running more than a couple of agents turns into its own ops problem: who calls whom, which key
does each agent hold, what did that call cost, why did it fail. Nasiko is a single control-plane
binary that sits in front of every agent and answers all of that.

- **Deploy anything that speaks A2A** — Python, Rust, Go, TypeScript, or any language with an
  HTTP server. `nasiko deploy` builds, pushes to the embedded registry, and runs it.
- **Routing engine** — a 3-stage pipeline (shortlist by embedding similarity → rerank on
  conversation context → LLM final pick) chooses the right agent for a query, so callers don't
  need to know your fleet.
- **Single ingress, always proxied** — the server terminates TLS, authenticates every request,
  and proxies all agent-to-agent traffic itself. Agents are never publicly reachable, and every
  hop is a checkpoint for rate limits, ACLs, and tracing.
- **MCP Gateway** — one permanent URL gives every agent a merged, permission-filtered view of
  Composio toolkits and generic MCP servers, without the agent ever holding the underlying
  credentials.
- **LLM Router** — agents get an `OPENAI_BASE_URL` and a short-lived Nasiko identity token
  instead of a real provider key. The router resolves the actual provider/model/key server-side
  and translates the request, so no agent — or its logs — ever sees a real API key.
- **Full observability** — every dispatch and proxy hop emits a real OTel span, so a multi-agent
  interaction is one trace end-to-end. Token usage and cost land automatically from `gen_ai.*`
  attributes.
- **Flow guards** — Redis-backed cascade limits (depth, fan-out, token budget, timeout, cycle
  detection) stop a runaway agent-calling-agent loop before it becomes an incident.
- **Encrypted secrets** — per-agent secrets are AES-256-GCM encrypted at rest and injected into
  the container only at deploy time.
- **Access control** — user→agent ownership/grants and an agent→agent allowlist gate every
  proxy call, independent of each other.
- **Embedded OCI registry** — `nasiko push`/`nasiko deploy` ship images straight to a
  self-hosted, S3-backed registry with layer dedup. No external registry required.
- **CLI-first, no lock-in** — `nasiko new`, `nasiko run`, `nasiko chat`, `nasiko deploy`. Bring
  your own LLM provider; the standard A2A protocol means no proprietary agent format.

For the full architecture, protocol, and platform guides, see the docs repo — this README covers
just the OSS build: what it is, and how to run it.

## Quick Start

### Prerequisites

- Docker (or Podman)
- Rust toolchain (stable, via [rustup](https://rustup.rs))
- [`just`](https://github.com/casey/just) (`cargo install just`)

### 1. Start infrastructure

```sh
just infra   # Postgres, Redis, MinIO (S3-compatible)
```

### 2. Configure environment

```sh
cp server/.env.example server/.env
# Edit server/.env — at minimum set OPENAI_API_KEY and SECRETS_ENCRYPTION_KEY
```

### 3. Run the platform

```sh
just run-stack   # infra + OSS server in one shot
# or, with infra already running:
just run
```

The server is the **sole ingress** — it terminates TLS, authenticates every request, proxies all
agent traffic, and serves the embedded UI, all from one process on `:8080`.

### 4. Install the CLI and deploy an agent

```sh
cargo build --release -p nasiko
sudo cp target/release/nasiko /usr/local/bin/

nasiko connect http://localhost:8080
nasiko login
nasiko new openai my-agent && cd my-agent
nasiko deploy .
nasiko chat "Hello"
```

## Architecture

```
                          ┌───────────────────────────────────────┐
   Client / CLI ───────► │            nasiko-server               │
                          │  CORS · tracing · auth · rate limiting │
                          │  ├─ API routes (agents, secrets, ...)  │
                          │  ├─ Routing engine (shortlist→rerank→  │
                          │  │   select)                          │
                          │  ├─ MCP Gateway (tools/list, tools/call)│
                          │  ├─ LLM Router (OpenAI-compatible)     │
                          │  ├─ Embedded OCI registry (/v2/*)      │
                          │  └─ Embedded UI (fallback route)       │
                          └──────────────────┬──────────────────────┘
                                             │ proxied A2A calls only
                                             ▼
                              Agent containers (Docker runtime)
```

There is no separate gateway process — every inter-agent call is proxied back through the server,
which is the single chokepoint where flow limits, ACLs, and observability are enforced. Durable
state lives in Postgres, Redis, and S3.

## Project Structure

```
server/         → Control plane: Axum routes, auth middleware, agent proxy, build worker, UI serving
orchestrator/   → Routing engine: semantic agent selection (shortlist → rerank → select)
react-agent/    → ReAct-loop LLM orchestrator (tool calls to agents)
mcp-gateway/    → MCP Gateway: connectors, tool aggregation, per-agent permissions, OAuth 2.1
llm-router/     → Provider-agnostic OpenAI-compatible egress proxy for agent LLM calls
runtime/        → ContainerRuntime trait + DockerRuntime (bollard)
auth/           → AuthService trait + OSS implementation (JWT login, RBAC hooks)
oidc/           → Generic OIDC relying-party client (works with any OIDC IdP)
flow/           → FlowGuard: anti-DoS cascade limits + live flow events
secrets/        → AES-256-GCM encryption for agent secrets at rest
oci/            → Embedded OCI Distribution v2 registry (S3-backed, layer dedup)
observability/  → OTel init, Tempo/Loki clients, DB-backed model pricing
agent-proxy/    → Agent ID → running container endpoint resolution
github/         → GitHub OAuth + repo import for source-based deploys
types/          → A2A protocol + registry types
config/         → Single env-driven Config struct
utils/          → Shared helpers
cli/            → `nasiko` binary (agent developer CLI, sync HTTP via ureq)
agents/         → Example and seed agents (each a standalone A2A container)
migrations/     → Postgres migrations (sqlx, run automatically at startup)
ui/             → Frontend (vanilla JS web components, embedded in the server binary)
docs/           → Design docs (architecture, protocol, conventions)
```

## Key CLI Commands

| Command | Description |
|---------|-------------|
| `nasiko up` / `nasiko down` | Start / stop the local Nasiko stack |
| `nasiko connect <url>` | Register a control plane and switch to it |
| `nasiko login` | Authenticate with the active cluster |
| `nasiko new [template] [name]` | Scaffold a new agent project |
| `nasiko build` / `nasiko run` | Build the agent image / build + run it locally |
| `nasiko deploy <image>` | Build, push, and deploy to the active cluster |
| `nasiko upload [source]` | Upload source (no local Docker needed); server builds it |
| `nasiko ps` | List running agents |
| `nasiko logs <agent> -f` | Stream (and follow) agent logs |
| `nasiko chat <agent>` | Interactive or one-shot A2A chat |
| `nasiko scale <agent> <n>` | Scale an agent to N replicas |
| `nasiko secrets set` | Configure encrypted per-agent secrets |
| `nasiko mcp` | Manage MCP Gateway connectors and tool permissions |
| `nasiko registry` | Browse the artifact registry |

Run `nasiko --help` for the full, workflow-ordered command list.

## Environment Variables

Every setting is env-driven through a single `Config` struct (`config/src/lib.rs`); required keys
fail fast at startup. Highlights:

| Variable | Purpose | Default |
|----------|---------|---------|
| `DATABASE_URL` | Postgres connection | required |
| `REDIS_URL` | Redis connection | required |
| `S3_*` | Object storage for the OCI registry | required |
| `SECRETS_ENCRYPTION_KEY` | Base64, 32-byte key for agent secret encryption | required |
| `AGENT_RUNTIME` | Container runtime (`docker` in OSS) | `docker` |
| `OPENAI_API_KEY` / `OPENAI_BASE_URL` | LLM provider for the router + orchestrator | optional |
| `ROUTER_MODEL` / `EMBEDDING_MODEL` | Models used by the routing engine | see `config/` |
| `FLOW_MAX_DEPTH` / `FLOW_MAX_FAN_OUT` / `FLOW_MAX_TOKENS` / `FLOW_TIMEOUT_SECS` | Flow guard cascade limits | see `config/` |
| `SEED_AGENTS` | Space-separated images auto-deployed at boot | optional |
| `ADMIN_USERNAME` / `ADMIN_PASSWORD` | Bootstrap admin account | required |
| `CORS_ALLOWED_ORIGINS` | CORS allowlist (empty is fine — UI is same-origin) | optional |

See `server/.env.example` for the full list.

## Building & Testing

```sh
cargo build --release -p nasiko          # CLI
cargo build --release -p nasiko-server   # Server

cargo check --workspace                  # Type check everything
cargo clippy                             # Lint (zero-warnings policy)

cargo test --workspace                   # Unit tests (hermetic, no infra needed)
# Integration tests need infra running:
just infra && just test
```

## Documentation

Deep design docs live in [`docs/`](docs/) — architecture, the A2A protocol, agent lifecycle, MCP
Gateway internals, CLI design, and networking. Hosted product docs (quickstart, dashboard,
platform guides) live in the separate docs repo.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for local setup, code conventions, and the PR flow.

## License

Apache-2.0
