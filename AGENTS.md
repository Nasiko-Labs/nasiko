# Nasiko project context

Nasiko is an AI agent runtime with a Rust control plane, a CLI, and a React dashboard.
The Cargo workspace has 33 members. The dashboard is embedded in the server binary;
the full Docker stack does not need a separate frontend container.

## Code map

| Location | Responsibility |
| --- | --- |
| `server/src/main.rs` | Startup: load configuration, connect Postgres, construct backends, run migrations, initialize state, serve HTTP. |
| `server/src/lib.rs` | Assemble Axum routes, authentication, rate limits, OCI registry, LLM and MCP gateways, and SPA fallback. |
| `server/src/state.rs` | Shared `AppState`, migrations, admin bootstrap, seed jobs, and background services. |
| `server/src/agents/`, `server/src/build/`, `server/src/catalog/` | Agent registration, deployment, builds, source uploads, imports, and lifecycle. |
| `server/src/agents/deploy_env.rs` | Shared assembly of agent deployment environment and platform credentials. |
| `server/src/router/`, `server/src/chat/`, `server/src/sessions/`, `server/src/coding/` | A2A dispatch, chat history, coding harness sessions, and streamed turns. |
| `server/src/evals/`, `evals/` | Durable replay evaluation jobs and task harvesting, gates, grading, and reports. |
| `config/`, `utils/`, `types/` | Environment configuration, shared helpers, A2A and registry protocol types. |
| `auth/`, `secrets/`, `session-token/` | User authentication, AES-256-GCM secret encryption, and session-bound coding tokens. |
| `runtime/`, `agent-proxy/` | Runtime and sandbox traits, Docker implementations, and agent endpoint resolution. |
| `acp-client/`, `harness-shim/`, `coding-policy/` | Coding harness protocol client, sandbox WebSocket/stdio bridge, and coding policy. |
| `orchestrator/` | Semantic agent selection, history/context selection, and multi-agent workflows. |
| `react-agent/` | ReAct loop used for routed dashboard chat. |
| `llm-router/` | Provider routing, model/key resolution, protocol adapters, optimization, and usage recording. |
| `mcp-gateway/`, `hitl/` | Tool aggregation, connector credentials, OAuth, permissions, and human approvals. |
| `flow/`, `redis/`, `leader/` | Cascade limits and flow events, shared Redis, and Postgres advisory locks for background jobs. |
| `oci/`, `github/` | S3-backed OCI registry and GitHub OAuth/source imports. |
| `observability/`, `pricing/`, `savings/`, `event-store/` | Tempo/Loki integration, normalized cost calculation, savings ledger, and Lance cold event storage. |
| `compress/`, `tool-compact/` | Payload compression and compact tool-schema encoding/decoding. |
| `spa/` | Serve embedded frontend assets. |
| `cli/` | `nasiko` command; synchronous HTTP client and commands under `src/commands/`. |
| `bench/`, `bench-support/` | Load generation and shared benchmark fixtures. |
| `agents/` | Standalone example A2A agents and coding harness images. |
| `migrations/` | SQLx migrations applied automatically during startup. |
| `ui/oss/app/` | OSS React application and Vite entry point. |
| `ui/shared/` | Shared app core/shell, feature screens, data access, UI components, mocks, and test helpers. |

The frontend uses React 19, Vite, TanStack Router/Query, Tailwind, and pnpm workspaces.
Feature screens live in `ui/shared/features/src/`; routes live in
`ui/shared/app-shell/src/routes/`. See `ui/package.json` for the pinned pnpm version.

## Request paths

- Dashboard and CLI call the authenticated REST API under `/api`.
- `POST /api/orchestrator/a2a` dispatches to a named agent or the ReAct orchestrator.
  The semantic shortlist/rerank/selection pipeline is also used to assign workflow steps;
  it is distinct from the ReAct chat loop. See `docs/ROUTING_ENGINE.md`.
- Deployed agents use the LLM router and `/api/mcp`; platform wiring injects their
  scoped credentials. Keep provider keys on the server.
- Postgres holds application state, Redis holds ephemeral coordination/cache state,
  RustFS provides S3 storage, and the OTel Collector exports to Tempo and Loki.

## Container setup

Run commands from the repository root. `docker-compose.yml` builds the UI and Rust
server using `server/Dockerfile` and starts seven services: server, Postgres with
pgvector, Redis, RustFS, OTel Collector, Tempo, and Loki.

For fast startup, use the pinned official server image with the prebuilt override:

```sh
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml up -d
```

This serves the published OSS revision recorded in `docker-compose.prebuilt.yml`.
Local source edits require a source build with the base Compose file. The local
source build was stopped after the published image passed dashboard, asset,
health, readiness, admin login, and authenticated API checks. Use the same two
Compose files for subsequent environment updates to keep using the published image.

The root `.env` is the container configuration. It already exists in this checkout:
preserve it and never print or commit its secret values. `.env.example` is a template.
`server/.env.example` documents additional server variables that can also be set in
the root `.env`. Compose `environment:` entries override `env_file:` values.

Required startup settings include `SECRETS_ENCRYPTION_KEY`, `JWT_SECRET`,
`ADMIN_PASSWORD`, and `S3_SECRET_KEY`; Compose supplies `DATABASE_URL` and `REDIS_URL`.
Set `ADMIN_USERNAME` for admin bootstrap. The encryption key must decode from base64
to exactly 32 bytes; generate it once with `openssl rand -base64 32` and preserve it
while the database contains encrypted secrets. Generate distinct signing keys with
`openssl rand -base64 48` for `JWT_SECRET` and `AGENT_JWT_SECRET`.

Set a real `OPENAI_API_KEY` in the root `.env` for the default provider. The dashboard
can boot without a working provider key; model calls need valid provider credentials.
Custom providers and per-user LLM configurations are supported. Optional Composio
and GitHub OAuth credentials enable their integrations.

This checkout uses the native Gemini provider: `GEMINI_API_KEY`,
`DEFAULT_PROVIDER=gemini`, and `DEFAULT_MODEL` / `OPENAI_MODEL` set to
`gemini-4-hydrogen`. Its default endpoint is
`https://generativelanguage.googleapis.com/v1beta`; an `OPENAI_BASE_URL` override
is unnecessary for native Gemini. Keep the Gemini key out of `OPENAI_API_KEY`.
The key and model availability were verified through Google's model-list API.
Run `python3 scripts/check-gemini.py` to verify actual text generation directly
through Google's native API and through Nasiko's existing provider-test endpoint.
The script reads `.env`, prints no credentials, and makes two small model requests.

The local `.env` sets `CARGO_BUILD_JOBS=1` after a parallel release build's
Lance compiler process was killed. Compose passes this setting into the Docker
build; the default is two jobs. BuildKit caches downloaded and compiled dependencies.

On this checkout, `.env` uses `POSTGRES_PORT=5433` because host Postgres uses `5432`.
This affects only the published host port: the server still connects to
`postgres:5432` on the Docker network. `POSTGRES_PORT` defaults to `5432` when unset.
The bundled RustFS credentials must match the `S3_ACCESS_KEY` and `S3_SECRET_KEY`
settings. Compose supplies all other internal service URLs.

```sh
docker compose config --quiet
docker compose up -d --build
docker compose ps --all
docker compose logs -f server
curl --fail http://localhost:8080/health
```

Open `http://localhost:8080`. Log in using `ADMIN_USERNAME` and `ADMIN_PASSWORD`
from the root `.env`. Reapply environment changes with `docker compose up -d`;
`restart` alone does not reload them. `docker compose down` stops the stack and
preserves named volumes; adding `-v` deletes stored data.

The Docker socket is mounted into the server so it can manage sibling agent
containers. The shared Docker network is named `nasiko`.

If image pulls fail with an `x509` certificate trust error, fix Docker Desktop's
certificate/proxy trust before retrying. This occurs before application startup
and cannot be fixed by changing Nasiko's JWT or provider keys. Docker documents
Mac certificate trust in its [Mac FAQ](https://docs.docker.com/desktop/troubleshoot-and-support/faqs/macfaqs/)
and [CA certificate guide](https://docs.docker.com/engine/network/ca-certs/).

## Native development and checks

`just infra` uses `docker-compose.infra.yml` for backing services. `just run` and
`just dev` source `server/.env` (falling back to `server/.env.example`). A native
server needs host URLs and the actual published Postgres port; do not reuse
Docker service names as host addresses. The infra-only Compose file currently
publishes Postgres on `5432` directly, so it also needs an override if that port
is occupied. Avoid running both Compose variants against the same project state.

Native Rust development requires Rust 1.88+ and `protoc`. Build the frontend to
serve the dashboard; `server/build.rs` can embed a placeholder for Rust-only
builds. Hot reload also needs `cargo-watch`. For frontend development, use
`pnpm install` in `ui/`, then `pnpm run dev:live oss` against a running server;
plain `pnpm run dev` uses mock data. Additional UI variables are documented in
`ui/.env.example`.

Common checks are `just check`, `just clippy`, and `just test-unit`. DB integration
checks are `just test-crates` and `just test-server` with compatible backing
services available. UI checks are `pnpm test`, `pnpm run lint`, and `pnpm run build`
from `ui/`. Choose checks relevant to the change.

Read `CONTRIBUTING.md`, `docs/ORGANIZATION.md`, and the applicable feature design
document before substantial changes. Some organization diagrams describe a
target structure; confirm current paths in the source.
