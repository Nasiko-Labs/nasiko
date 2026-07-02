# Nasiko

An open-source platform for deploying, managing, and orchestrating AI agents using the [A2A protocol](https://github.com/a2aproject/a2a-spec).

## What is Nasiko?

Nasiko gives you a control plane for AI agents. You build agents that speak A2A, deploy them with a single command, and the platform handles routing, orchestration, secrets, and observability.

- **Deploy any A2A agent** — Python, Rust, Go, or any language with an HTTP server
- **Multi-agent orchestration** — ReAct-loop orchestrator routes queries to the right agent
- **CLI-first workflow** — `nasiko deploy`, `nasiko chat`, `nasiko ps`
- **No vendor lock-in** — standard A2A protocol, bring your own LLM provider

## Quick Start

### Prerequisites

- Docker (or Podman)
- Rust toolchain (1.80+)

### 1. Install the CLI

```sh
cargo build --release -p nasiko
sudo cp target/release/nasiko /usr/local/bin/
```

### 2. Start infrastructure

```sh
just infra   # Postgres, Redis, S3 (MinIO/RustFS)
```

### 3. Configure environment

```sh
cp server/.env.example server/.env
cp gateway/.env.example gateway/.env
# Edit both .env files to set OPENAI_API_KEY
```

### 4. Run the platform

```sh
just run
```

Server starts on `:8080`, gateway on `:8443`.

### 5. Connect and chat

```sh
nasiko connect http://localhost:8443
nasiko login
nasiko chat http://localhost:8443/api/a2a "Hello"
```

## Architecture

```
Gateway (Axum)                         Server (Axum)
┌──────────────────────────┐           ┌─────────────────────────┐
│ • JWT auth + revocation  │           │ • All API routes        │
│ • Rate limiting          │  HTTP     │ • Business logic        │
│ • A2A agent proxying     │ ────────► │ • DB/Redis/OCI          │
│ • Orchestrator (ReAct)   │           │ • Trusts gateway headers│
│ • Flow guards            │           └─────────────────────────┘
└──────────────────────────┘
         │
         ▼
   Agent containers (A2A protocol)
```

## Project Structure

```
cli/              → `nasiko` binary (agent developer CLI)
server/           → API server (all routes, business logic)
gateway/          → Auth, orchestrator, agent proxy, flow guards
auth/             → AuthService trait + implementations
config/           → Environment-based configuration
runtime/          → ContainerRuntime trait + DockerRuntime
react-agent/      → ReAct-loop LLM orchestrator
types/            → A2A protocol types
oci/              → OCI Distribution (image push/pull)
flow/             → Flow tracking + cascade protection
secrets/          → AES-256-GCM secret encryption
agent-proxy/      → Agent endpoint resolution
observability/    → OpenTelemetry integration
github/           → GitHub OAuth + repo integration
utils/            → Shared utilities
agents/           → Example agents (echo, nutrition, docs, paper)
migrations/       → Postgres migrations
ui/               → Frontend (vanilla JS web components)
```

## Key Commands

| Command | Description |
|---------|-------------|
| `nasiko connect <url>` | Add and switch to a cluster |
| `nasiko login` | Authenticate with the cluster |
| `nasiko deploy` | Build image and deploy agent |
| `nasiko ps` | List running containers |
| `nasiko chat <url>` | A2A chat (interactive or one-shot) |
| `nasiko logs <agent>` | Stream agent logs |
| `nasiko secrets set` | Configure agent secrets |

## Environment Variables

Key configuration (see `config/src/lib.rs` for full list):

| Variable | Purpose | Default |
|----------|---------|---------|
| `DATABASE_URL` | Postgres connection | required |
| `REDIS_URL` | Redis connection | required |
| `S3_ENDPOINT` | Object storage | `http://localhost:9000` |
| `OPENAI_API_KEY` | LLM for orchestrator + agents | optional |
| `OPENAI_BASE_URL` | OpenAI-compatible endpoint | optional |
| `OPENAI_MODEL` | Model for orchestrator | `deepseek-v4-flash` |
| `SEED_AGENTS` | Space-separated images to auto-deploy | optional |
| `JWT_SECRET` | Token signing secret | required |
| `ADMIN_USERNAME` | Bootstrap admin username | `admin` |
| `ADMIN_PASSWORD` | Bootstrap admin password | required |

## Building

```sh
cargo build --release -p nasiko          # CLI
cargo build --release -p nasiko-server   # Server
cargo build --release -p nasiko-gateway  # Gateway
```

## Running Tests

```sh
cargo test --workspace
```

## Documentation

- [Architecture](../docs/ARCHITECTURE.md)
- [CLI Design](../docs/CLI_DESIGN.md)
- [Agent Development Lifecycle](../docs/ADLC.md)
- [A2A Registry Design](../docs/A2A_REGISTRY_DESIGN.md)
- [Bootstrap & Networking](../docs/BOOTSTRAP_AND_NETWORKING.md)

## License

Apache-2.0
