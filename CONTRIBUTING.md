# Contributing to Nasiko

Thanks for your interest in Nasiko. This guide explains what we accept, how a pull request is
handled, and the checks a change must pass.

- Documentation: [docs.nasiko.com](https://docs.nasiko.com)
- Questions and discussion: [Discord](https://discord.com/invite/HmnfkTfjFv)
- Bugs and proposals: [GitHub issues](https://github.com/Nasiko-Labs/nasiko/issues)

## Read this first

We welcome contributions, and our review time is limited. Opening a pull request does not oblige us
to review or merge it: we may close or defer it, ask for a smaller scope, or solve the problem a
different way. Following this guide makes a pull request eligible for review; it does not
guarantee acceptance.

Focused bug fixes, reliability and performance improvements, missing tests, and documentation
corrections are the most likely to be accepted. Unrequested features, broad rewrites, and unrelated
cleanups are not.

## Agree on the problem before you build

- **Small, obvious bug fix:** open the pull request directly. Explain the defect and why your fix
  is correct.
- **Larger bug fix:** link an issue that describes the failure and the expected behaviour.
- **New feature or change in product behaviour:** open an issue first and wait for a maintainer to
  agree on the direction and scope. Link that agreement in your pull request. Describing a
  behaviour change as a bug fix does not skip this step.

Search existing issues and pull requests before you start, so two people do not build the same
thing.

## One problem per pull request

A pull request solves one underlying problem. Include everything that problem needs (code, tests,
documentation, migrations) and nothing else. A second fix, a refactor, or a cleanup belongs in its
own pull request unless the first cannot work without it. A large change is fine; an unrelated one
is not.

## Show that it works

In the pull request description, explain:

- how you reproduced the problem,
- what you changed and why,
- the exact commands or manual steps you used to check the change, and what you observed,
- anything you could not check.

"Tests pass" on its own is not evidence. A bug fix needs a test that fails before the fix and
passes after it. A UI change needs before and after screenshots, plus a short recording when motion
or timing matters. Attach them to the pull request; do not commit them to the repository.

## What happens after you open a pull request

1. Your pull request is picked up automatically, and checks run for the parts of the code it
   touches: `cargo test` for the affected crates, and type checking, linting, build and tests when
   you change `ui/`.
2. The result appears on your pull request as a commit status and a comment that is kept up to
   date as you push. If your branch no longer applies cleanly, you are asked to rebase.
3. When several pull requests solve the same problem, maintainers compare how reviewable they are
   (tests, size, how many areas they touch, documentation) and keep one.
4. A merged pull request keeps your original commits and authorship.
5. A pull request that is closed without merging always gets a comment with the reason: superseded
   by another pull request, duplicate, out of scope, failing checks, needs a rebase, or stale.
   Address the reason and you are welcome to open a new pull request.

## Getting started

You need Rust 1.85 or newer (the workspace uses edition 2024), Docker or Podman, and
[`just`](https://github.com/casey/just). UI work also needs Node.js 22.18 or newer and pnpm.

```sh
just infra                             # start Postgres, Redis, RustFS and the OTel stack in Docker
cp server/.env.example server/.env     # annotated reference for every server variable
just dev                               # run the server natively with hot-reload (needs cargo-watch)
# ...or without hot-reload:
just run
```

The server runs on [http://localhost:8080](http://localhost:8080). Edit `server/.env` for anything
beyond the defaults. For a host-run server, set `CODING_AGENT_OTLP_ENDPOINT=http://localhost:4318`
there if you need coding-agent traces and cost.

Useful while developing:

```sh
just check                               # cargo check --workspace
just clippy                              # lint (zero-warnings policy)
cargo build --release -p nasiko          # CLI binary
cargo build --release -p nasiko-server   # server binary
```

### Installing the toolchain

**macOS**

```sh
xcode-select --install                                           # C toolchain and linker
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust
source "$HOME/.cargo/env"
cargo install just cargo-watch
```

**Linux (Debian/Ubuntu)**

```sh
sudo apt update && sudo apt install -y build-essential pkg-config libssl-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
cargo install just cargo-watch
# Docker engine + Compose plugin: https://docs.docker.com/engine/install/ubuntu/
```

**Windows**

```powershell
winget install --id Rustlang.Rustup -e
# reopen the terminal, then:
cargo install just cargo-watch
```

If native crates such as `ring` fail to link on the MSVC toolchain (`link.exe not found`), install
the C++ build tools:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

A `dlltool ... Invalid bfd target` error means a 32-bit MinGW (`C:\MinGW`) is on `PATH`. Install
64-bit MinGW-w64 (for example through [MSYS2](https://www.msys2.org/)) or switch to MSVC.

Check everything is in place:

```sh
rustc --version && cargo --version && just --version
docker --version && docker compose version
```

The first `cargo` build compiles the whole workspace and is slow. Use a native clone rather than a
mounted or 9p filesystem.

### Where code lives

```
server/         Control plane: Axum routes, auth, agent proxy, build worker, embedded UI
orchestrator/   Routing engine: semantic agent selection (shortlist, rerank, select)
mcp-gateway/    MCP Gateway: connectors, tool aggregation, per-agent permissions, OAuth
llm-router/     Provider-agnostic OpenAI-compatible egress proxy for agent LLM calls
runtime/        ContainerRuntime trait + DockerRuntime (bollard)
auth/           AuthService trait + OSS implementation (JWT login, RBAC hooks)
flow/           FlowGuard: anti-DoS cascade limits + live flow events
secrets/        AES-256-GCM encryption for agent secrets at rest
oci/            Embedded OCI Distribution v2 registry (S3-backed, layer dedup)
observability/  OTel init, Tempo/Loki clients, DB-backed model pricing
agent-proxy/    Agent ID -> running-container endpoint resolution
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

See [`docs/ORGANIZATION.md`](docs/ORGANIZATION.md) for more on how the code is organized.

## Before you push

All of these must pass with zero warnings:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
just test-unit
```

Then run the integration tests for the area you changed. For changes under `ui/`, run
`pnpm install`, `pnpm typecheck`, `pnpm lint` and `pnpm test` inside `ui/`.

| Command | Needs | Covers |
|---|---|---|
| `just test-unit` | nothing | Hermetic unit tests: no network, database or Docker |
| `just test-crates` | `just infra` | Library integration tests that need Postgres |
| `just test-server` | `just infra` | Server integration tests |
| `just test-one <name>` | `just infra` | One server test file, for example `just test-one auth_flow` |
| `just test` | `just infra` | All of the above |

Unit tests must stay hermetic. Integration tests share one database, so they run one at a time.

## Code standards

The binding guides live in [`docs/`](docs):

- [`CLEAN_CODE_GUIDE.md`](docs/CLEAN_CODE_GUIDE.md): names, function size, error handling, traits
  and tests
- [`ORGANIZATION.md`](docs/ORGANIZATION.md): where code belongs
- [`API_CONVENTIONS.md`](docs/API_CONVENTIONS.md): the wire format of every HTTP endpoint
- [`A2A_PROTOCOL.md`](docs/A2A_PROTOCOL.md): how agents talk to the platform

The rules that most often block a pull request:

- **Zero warnings.** `cargo check` and `cargo clippy` report nothing, and the code is formatted with
  `cargo fmt`.
- **Dependencies are declared once,** in the root `Cargo.toml`; crates use `dep.workspace = true`.
  Explain why a new dependency is needed when an existing one cannot do the job.
- **Extend through traits.** Seams such as `AuthService`, `ContainerRuntime`, `RoutingEngine` and
  `ObservabilityProvider` are wired once at startup, and handlers receive `Arc<dyn Trait>`.
- **The CLI stays lightweight:** synchronous HTTP with `ureq`, no async runtime.
- **Migrations are new files.** Add a file named `NNNN_short_description.sql` in `migrations/`,
  numbered one above the highest existing file. Migrations run automatically at startup; never edit
  one that has already been released.
- **No secrets,** not even in examples, fixtures or tests.

## Reporting bugs

Open an [issue](https://github.com/Nasiko-Labs/nasiko/issues) with:

- what you expected to happen,
- what happened instead,
- the steps to reproduce it,
- your operating system, Rust version, and Nasiko version or commit.

Logs help; remove any secrets from them first.

## Security issues

Do not report a vulnerability in a public issue or pull request. Contact the maintainers privately
on [Discord](https://discord.com/invite/HmnfkTfjFv) and we will follow up with you directly.

## License

Nasiko is licensed under the [Apache License 2.0](LICENSE). By submitting a contribution, you agree
that it is licensed under the same terms.
