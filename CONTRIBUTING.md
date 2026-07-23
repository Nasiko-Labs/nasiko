# Contributing to Nasiko

Thanks for your interest in contributing! This guide covers development setup, the build/test
commands, and the PR flow.

## Development Setup

### Prerequisites

- Rust (stable, via [rustup](https://rustup.rs))
- Docker or Podman (the `justfile` auto-detects Podman)
- [`just`](https://github.com/casey/just) command runner (`cargo install just`)

### Getting Started

```sh
# Start backing infrastructure (Postgres, Redis, MinIO)
just infra

# Configure environment (just run falls back to the example if you skip this)
cp server/.env.example server/.env

# Run the server (single binary — it terminates TLS, validates auth, serves the UI)
just run
```

The server is the sole ingress: there is no separate gateway process.

### Install the CLI

```sh
cargo build --release -p nasiko
sudo cp target/release/nasiko /usr/local/bin/
```

### Useful Commands

```sh
just run              # Server (foreground; sources server/.env)
just infra            # Start Postgres, Redis, MinIO
just infra-down       # Stop infrastructure
just logs             # View infra logs (-f to follow)
just check            # cargo check --workspace
just clippy           # Lint
```

## Code Conventions

### Zero Warnings

The workspace must compile with zero warnings from `cargo check` and `cargo clippy`. CI fails
on warnings; run `cargo fmt` before committing.

### Single Root Workspace

All dependencies are declared once in the root `Cargo.toml`. Crates use `dep.workspace = true`.

### Architecture Principles

- **The server is the single ingress** — it validates JWTs itself (the `AuthService` trait),
  runs the middleware stack (CORS, tracing, auth, rate limiting), proxies all agent traffic,
  and serves the embedded UI. Agents are never publicly exposed.
- **Trait-based extensibility** — `AuthService`, `ContainerRuntime`, `RoutingEngine`,
  `ObservabilityProvider` are traits with pluggable implementations, wired once at startup;
  handlers receive `Arc<dyn Trait>`.
- **CLI stays lightweight** — the `nasiko` binary uses `ureq` (sync HTTP), no tokio, for fast
  compile times.
- **UI is vanilla JS** — web components, no build step, no framework; embedded in the server
  binary and served same-origin.

### A2A Protocol

Agents implement the [A2A protocol v1.0](https://github.com/a2aproject/a2a-spec). Key details:

- Method names: `SendMessage`, `SendStreamingMessage` (gRPC-style)
- Role enum: `ROLE_USER`, `ROLE_AGENT`
- Header: `A2A-Version: 1.0` required
- Parts: `[{"text": "..."}]` (no `kind` field in v1.0)

### Database Migrations

Migrations live in `migrations/` and run automatically on server startup via sqlx.

To add a migration:
```sh
sqlx migrate add -r <description>
```

## Project Layout

```
cli/           → `nasiko` binary (developer CLI)
server/        → Axum control plane: routes, auth middleware, agent proxy, build worker, UI serving
auth/          → AuthService trait + impls (login, JWT validation, RBAC hooks)
oidc/          → OIDC SSO client
config/        → Config struct (env-driven)
runtime/       → ContainerRuntime trait + DockerRuntime
orchestrator/  → Routing engine: semantic agent selection (shortlist → rerank → select)
react-agent/   → ReAct orchestrator (LLM + tool calls to agents)
mcp-gateway/   → MCP gateway: connectors, per-agent tool permissions, delegation
types/         → A2A protocol types (wraps the a2a crate)
oci/           → Embedded OCI Distribution registry (S3-backed)
flow/          → FlowGuard: anti-DoS cascade limits, flow events
secrets/       → AES-256-GCM encryption for agent secrets
agent-proxy/   → Agent endpoint resolution
observability/ → OpenTelemetry init, Tempo/Loki providers
github/        → GitHub OAuth + source integration
utils/         → Shared helpers
agents/        → Example/seed agents (each a standalone A2A container)
migrations/    → SQL migrations
ui/            → Frontend (vanilla JS web components)
```

## Testing

### Unit Tests (hermetic — no network, DB, or Docker)

```sh
just test-unit
```

### Integration Tests

Server integration tests need infra up and run serially:

```sh
just infra
just test-server           # all server integration tests
just test-one auth_flow    # a single test file
just test                  # unit + server integration
```

## Pull Request Guidelines

1. `cargo fmt`, then ensure `cargo check --workspace` and `cargo clippy` pass with zero warnings
2. Add tests for new functionality; keep unit tests hermetic
3. Keep PRs focused — one logical change per PR
4. Write a clear description of what and why

## Reporting Issues

Open an issue on GitHub with:
- What you expected to happen
- What actually happened
- Steps to reproduce
- Rust version and OS
