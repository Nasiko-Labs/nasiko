# Contributing to Nasiko

Thanks for your interest in contributing! This guide covers the development setup and conventions.

## Development Setup

### Prerequisites

- Rust 1.80+ (install via [rustup](https://rustup.rs))
- Docker or Podman
- `just` command runner (`cargo install just`)

### Getting Started

```sh
# Start backing infrastructure
just infra

# Copy and configure environment
cp server/.env.example server/.env
cp gateway/.env.example gateway/.env

# Run the platform
just run
```

### Install the CLI

```sh
cargo build --release -p nasiko
sudo cp target/release/nasiko /usr/local/bin/
```

### Useful Commands

```sh
just run              # Server + gateway (foreground)
just run-server       # Server only
just run-gateway      # Gateway only
just infra            # Start Postgres, Redis, S3
just infra-down       # Stop infrastructure
just logs             # View infra logs
```

## Code Conventions

### Zero Warnings

The workspace must compile with no warnings. CI will fail on warnings.

### Single Root Workspace

All dependencies are declared once in the root `Cargo.toml`. Crates use `dep.workspace = true`.

### Architecture Principles

- **Gateway owns auth** — the server trusts gateway-injected headers and never validates JWTs itself
- **Trait-based extensibility** — `AuthService`, `ContainerRuntime`, `ObservabilityProvider` are all traits with pluggable implementations
- **CLI stays lightweight** — the `nasiko` binary uses `ureq` (sync HTTP), no tokio, for fast compile times
- **UI is vanilla JS** — web components, no build step, no framework

### Auth Model

The `AuthService` trait is the single interface for all auth operations:

```rust
pub trait AuthService: Send + Sync + 'static {
    async fn validate_token(&self, token: &str) -> Result<Identity, AuthError>;
    async fn issue_token(&self, identity: &Identity) -> Result<String, AuthError>;
    async fn authenticate(&self, username: &str, password: &str) -> Result<LoginResult, AuthError>;
    // ... revocation, ACL, etc.
}
```

OSS uses `AuthServiceImpl` (DB-backed). The gateway uses `SimpleJwtAuth` for token-only validation.

### A2A Protocol

Agents implement the [A2A protocol v1.0](https://github.com/a2aproject/a2a-spec). Key details:

- Method names: `SendMessage`, `SendStreamingMessage` (gRPC-style)
- Role enum: `ROLE_USER`, `ROLE_AGENT`
- Header: `A2A-Version: 1.0` required
- Parts: `[{"text": "..."}]` (no `kind` field in v1.0)

### Database Migrations

Migrations live in `migrations/`. They run automatically on server startup via sqlx.

To add a migration:
```sh
sqlx migrate add -r <description>
```

## Project Layout

```
cli/           → `nasiko` binary
server/        → Axum API server (routes, business logic)
gateway/       → Auth middleware, orchestrator, agent proxy
auth/          → AuthService trait + impls
config/        → Config struct (from env)
runtime/       → ContainerRuntime trait + DockerRuntime
react-agent/   → ReAct orchestrator (LLM + tool calls to agents)
types/         → A2A protocol types (re-exports a2a-lf crate)
oci/           → OCI Distribution spec implementation
flow/          → Flow tracking, cascade protection
secrets/       → AES-256-GCM encryption for agent secrets
agent-proxy/   → Agent endpoint resolution
observability/ → OpenTelemetry init + provider trait
github/        → GitHub OAuth + source integration
utils/         → Shared helpers
agents/        → Example/seed agents
migrations/    → SQL migrations
ui/            → Frontend (vanilla JS web components)
```

## Testing

### Unit Tests

```sh
cargo test --workspace
```

### Integration Tests

Integration tests in `server/tests/` require a running Postgres instance:

```sh
just infra
cargo test -p nasiko-server --test auth_flow
```

### Testing Agents Locally

The echo-agent is the simplest test target (no LLM required):

```sh
# Deploy seed agents
SEED_AGENTS="akhilfolium/echo-agent" just run

# Chat directly
nasiko chat http://localhost:<port>/ "Hello"
```

## Pull Request Guidelines

1. Ensure `cargo check --workspace` passes with zero warnings
2. Add tests for new functionality
3. Keep PRs focused — one logical change per PR
4. Write a clear description of what and why

## Reporting Issues

Open an issue on GitHub with:
- What you expected to happen
- What actually happened
- Steps to reproduce
- Rust version and OS
