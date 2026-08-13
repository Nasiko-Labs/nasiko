# Nasiko

**The open-source control plane for AI agents.** Deploy any [A2A](https://github.com/a2aproject/a2a-spec)-speaking
agent with one command and get routing, tool access, secrets, and observability out of the box.

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

## Quick Start

The only prerequisite is **Docker** (with Docker Compose)

### 1. Clone and configure

```sh
git clone https://github.com/Nasiko-Labs/nasiko.git
cd nasiko
cp .env.example .env
```

Edit `.env` and set at minimum:

- `OPENAI_API_KEY` — your OpenAI key (used by the routing engine and injected into agents)
- `ADMIN_PASSWORD` — password for the bootstrap admin account

The rest has working defaults for local development.

### 2. Start the platform

```sh
docker compose up -d
```

This builds the server from source and starts the full stack: Postgres, Redis, S3-compatible
storage (RustFS), OpenTelemetry collectors (Tempo + Loki), and the Nasiko control-plane server.

The first build takes a few minutes while Rust compiles all dependencies. Subsequent builds are
fast — cargo's registry, git index, and compiled artifacts are cached across builds.

Once running, open [http://localhost:8080](http://localhost:8080) for the dashboard.

```sh
docker compose logs -f server   # follow server logs
docker compose down             # stop everything
docker compose up -d --build    # rebuild after pulling new changes
```

### 3. Deploy your first agent

Install the CLI `cargo install --path cli/`, or 
build from source (`cargo build --release -p nasiko`).

```sh
nasiko connect http://localhost:8080
nasiko auth login                          # log in with ADMIN_USERNAME / ADMIN_PASSWORD
nasiko new openai my-agent && cd my-agent  # scaffold from a template
nasiko deploy .                            # build, push, and deploy
nasiko chat "Hello"                        # talk to your agent
```

You can also deploy agents directly from the dashboard UI — upload source, import from GitHub,
or pull from the artifact registry.

## Development Setup

For contributors or anyone wanting faster iteration with hot-reload. Requires:

- [Rust toolchain](https://rustup.rs) (stable)
- [`just`](https://github.com/casey/just) (`cargo install just`)
- Docker (for infrastructure services)

```sh
# 1. Start infrastructure only (Postgres, Redis, RustFS, OTel stack)
just infra

# 2. Configure the server
cp server/.env.example server/.env
# Edit server/.env — set OPENAI_API_KEY at minimum

# 3. Run the server natively (with hot-reload)
just dev

# Or without hot-reload:
just run
```

The server runs on `http://localhost:8080`. Source changes to `server/` trigger an automatic
rebuild when using `just dev` (requires
[`cargo-watch`](https://github.com/watchexec/cargo-watch)).

### Useful dev commands

```sh
just check                # cargo check --workspace
just clippy               # lint (zero-warnings policy)
just test-unit            # fast hermetic unit tests (no infra needed)
just test                 # unit + integration tests (needs just infra)
just fmt                  # cargo fmt
```

### Building from source

```sh
cargo build --release -p nasiko          # CLI binary
cargo build --release -p nasiko-server   # Server binary
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
server/         Control plane: Axum routes, auth middleware, agent proxy, build worker, UI
orchestrator/   Routing engine: semantic agent selection (shortlist → rerank → select)
react-agent/    ReAct-loop LLM orchestrator (tool calls to agents)
mcp-gateway/    MCP Gateway: connectors, tool aggregation, per-agent permissions, OAuth 2.1
llm-router/     Provider-agnostic OpenAI-compatible egress proxy for agent LLM calls
runtime/        ContainerRuntime trait + DockerRuntime (bollard)
auth/           AuthService trait + OSS implementation (JWT login, RBAC hooks)
flow/           FlowGuard: anti-DoS cascade limits + live flow events
secrets/        AES-256-GCM encryption for agent secrets at rest
oci/            Embedded OCI Distribution v2 registry (S3-backed, layer dedup)
observability/  OTel init, Tempo/Loki clients, DB-backed model pricing
agent-proxy/    Agent ID → running container endpoint resolution
github/         GitHub OAuth + repo import for source-based deploys
types/          A2A protocol + registry types
config/         Single env-driven Config struct
utils/          Shared helpers
cli/            nasiko binary (agent developer CLI, sync HTTP via ureq)
agents/         Example and seed agents (each a standalone A2A container)
migrations/     Postgres migrations (sqlx, run automatically at startup)
ui/             Frontend (vanilla JS web components, embedded in the server binary)
docs/           Design docs (architecture, protocol, conventions)
```

## CLI Commands

| Command | Description |
|---------|-------------|
| `nasiko connect <url>` | Register a control plane and switch to it |
| `nasiko auth login` | Authenticate with the active cluster |
| `nasiko new [template] [name]` | Scaffold a new agent project |
| `nasiko deploy <path>` | Build, push, and deploy to the active cluster |
| `nasiko upload [source]` | Upload source for server-side build (no local Docker needed) |
| `nasiko ps` | List running agents |
| `nasiko logs <agent> -f` | Stream agent logs |
| `nasiko chat <agent>` | Interactive or one-shot A2A chat |
| `nasiko scale <agent> <n>` | Scale an agent to N replicas |
| `nasiko secrets set` | Configure encrypted per-agent secrets |
| `nasiko mcp` | Manage MCP Gateway connectors and tool permissions |

Run `nasiko --help` for the full command list.

## Environment Variables

Every setting is env-driven through a single `Config` struct (`config/src/lib.rs`). Required keys
fail fast at startup. When running via `docker compose`, infrastructure URLs (`DATABASE_URL`,
`REDIS_URL`, `S3_ENDPOINT`, etc.) are set automatically — see `docker-compose.yml`.

| Variable | Purpose | Default |
|----------|---------|---------|
| `OPENAI_API_KEY` | LLM provider for the router + agents | required |
| `SECRETS_ENCRYPTION_KEY` | Base64 32-byte AES-256-GCM key | required |
| `ADMIN_USERNAME` / `ADMIN_PASSWORD` | Bootstrap admin account | required |
| `DATABASE_URL` | Postgres connection | set by compose |
| `REDIS_URL` | Redis connection | set by compose |
| `S3_*` | Object storage for the OCI registry | set by compose |
| `AGENT_RUNTIME` | Container runtime (`docker` in OSS) | `docker` |
| `ROUTER_MODEL` / `EMBEDDING_MODEL` | Models used by the routing engine | see `config/` |
| `FLOW_MAX_DEPTH` / `FLOW_MAX_FAN_OUT` / `FLOW_MAX_TOKENS` | Flow guard cascade limits | see `config/` |
| `SEED_AGENTS` | Space-separated images auto-deployed at boot | optional |

See `.env.example` for the full list with descriptions.

## Documentation

Design docs live in [`docs/`](docs/) — architecture, the A2A protocol, agent lifecycle, MCP
Gateway internals, CLI design, and networking.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for local setup, code conventions, and the PR flow.

## License

Apache-2.0