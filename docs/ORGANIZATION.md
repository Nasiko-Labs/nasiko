# Code Organization — Target Standard

How this workspace should be laid out for readability and easy navigation. This is the
**target**: follow it for new code, and refactor toward it when you touch existing code.

The workspace has **three package types** — **libraries**, **servers**, **CLIs**. Every rule
below applies **per crate**. (The Nasiko enterprise edition extends these same crates by wrapping
their traits; it follows this standard too and never duplicates logic.)

---

## Guiding principles

1. **Flat over deep.** Group by layer, one file per feature (`handlers/chat.rs`, `repo/chat.rs`).
   A feature bigger than one file gets a subfolder/module (`handlers/agents/`).
2. **Two layers in the server, not three.** `handler → repo`. No service layer until the *same*
   logic is used by two handlers.
3. **The server is minimal.** It holds only handlers (HTTP) and repos (SQL). Any real
   engine/capability lives in a library crate the server calls.
4. **A trait only when there are two implementations** (or you genuinely mock it). One impl behind
   a trait is indirection with no payoff.
5. **Share types, not stacks.** Request/response types live in a serde-only `dto/` crate used by
   both server and CLI.

The traits that genuinely have two implementations (the enterprise edition supplies the second):

| Trait                   | Default impl                       |
| ----------------------- | ---------------------------------- |
| `AuthService`           | `auth` (JWT + SQLx)                |
| `ContainerRuntime`      | `runtime` (`DockerRuntime`)        |
| `RoutingEngine`         | `orchestrator` (`OssRoutingEngine`)|
| `ObservabilityProvider` | `observability` (Tempo + Loki)     |

---

## 1. Libraries

```
<lib>/
├── Cargo.toml
├── benches/<area>.rs        ← criterion, ONLY where a real hot path exists
├── examples/<scenario>.rs   ← one public-API use each; `cargo build --examples` in CI
├── tests/                   ← integration tests, one file per area
│   ├── <feature>.rs
│   └── common/mod.rs        ← shared fixtures
└── src/
    ├── lib.rs               ← public API: module decls + `pub use`, no logic
    ├── models.rs            ← this crate's structs/enums (→ models/ folder when >~400 lines)
    ├── error.rs             ← ONE thiserror enum (see §5)
    ├── <concern>.rs         ← one concern per file; unit tests inline (#[cfg(test)])
    └── repo/                ← ONLY if the lib does its own data access
```

- `lib.rs` re-exports so callers write `nasiko_oci::Storage`, not `..::storage::Storage`.
- A pre-existing `types.rs` == `models.rs` — don't rename for its own sake.
- **Unit tests** stay inline in each `src/*.rs`; **integration tests** go in `tests/`.
- **Libraries never read env** — they take config from the caller (keeps them reusable/testable).

---

## 2. Servers

The server is a thin HTTP + SQL layer. Request flow: `routes.rs → handlers/<f>.rs → repo/<f>.rs → DB`.

```
server/src/
├── main.rs                  ← thin: init telemetry → build AppState → serve
├── lib.rs                   ← build_app(): AppState + mount routes; calls seed() at startup
├── routes.rs                ← the API index: every path+verb+handler in ONE file
├── config.rs                ← server config (root of src/); composes the shared config crate
├── state.rs                 ← AppState (repos behind the repo trait, db pool, redis, config, runtime handles)
├── middleware.rs            ← single module: all cross-cutting layers (rate_limit, acl, auth)
├── handlers/                ← thin: extractors → repo calls → response
│   ├── mod.rs
│   ├── telemetry.rs         ← telemetry setup + genai metrics (lives under handlers/)
│   ├── chat.rs · obs.rs · proxy.rs · users.rs · build.rs · flows.rs
│   ├── admin.rs · auth.rs · secrets.rs · github.rs · usage.rs
│   ├── capabilities.rs · settings.rs · pool.rs · transcribe.rs
│   ├── agents/  (mod · update · upload · deployments)   ← large feature → subfolder
│   └── catalog/ (mod · browse · import · skills)        ← large feature → subfolder
├── repo/                    ← ALL sqlx here; mod.rs defines the repo trait/interface
│   ├── mod.rs               ← the repo trait(s) + re-exports; holds the seed() bootstrap fn
│   ├── chat.rs · users.rs · build.rs · flows.rs · admin.rs · auth.rs
│   ├── secrets.rs · github.rs · usage.rs · capabilities.rs · settings.rs · obs.rs
│   ├── agents/  (agents · grants · deployments · acl)
│   └── catalog/ (catalog · import · agent_secrets)
├── tests/                   ← integration; common/ fixtures; #[serial] DB tests
└── migrations/              ← NNNN_snake_case.sql, consistent zero-padded width
```

- **`routes.rs` is the API surface at a glance.** Handlers don't declare their own routes.
- **Handlers are thin:** parse extractors → call one or two repo methods → map to response.
- **`repo/<feature>.rs` owns all SQL for that feature.** `repo/mod.rs` declares the repo
  trait(s) — the data-access interface; each feature file implements it. The trait keeps a clean
  boundary between handlers and SQL and lets handlers be unit-tested against a mock repo.
- **`config.rs` at the root of `src/`** is the server's config entrypoint; it composes the shared
  `config` crate so env-loading isn't duplicated. Validate every field fail-fast at boot — no
  `std::env::var` in a handler.
- **Errors use `anyhow`**, mapped to an HTTP status at the handler boundary (see §5).
- **Bootstrap/seed is a normal `fn`** in `repo/mod.rs`, invoked from `build_app()` at startup.
- **All cross-cutting layers live in a single `middleware.rs`** (rate limiting, ACL, auth).
- **Telemetry setup lives under `handlers/`** (`handlers/telemetry.rs`).
- **Wire formats follow `docs/API_CONVENTIONS.md`** — envelopes, errors, status codes.

---

## 3. CLIs

Synchronous only — **no tokio**. `ureq` (HTTP), `clap` (args), `dialoguer`/`indicatif` (UX). Fast
compile, linear control flow.

```
cli/src/
├── main.rs                  ← fn main() -> ExitCode; define clap Cli/Commands; dispatch only
├── lib.rs · config.rs       ← config.rs = ~/.nasiko/config.json load/save
├── clients.rs               ← the single ureq HTTP client wrapper(s), speak dto types
├── output.rs                ← all rendering: table / plain / --json in one place
└── commands/                ← group related commands; multi-file group → subdir
    ├── mod.rs
    ├── auth/                ← feature cluster (subdir when a group has multiple files)
    ├── agents/  (deploy · build · push · upload · card · scaffold · validate)
    ├── chat/    (app · event · session · ui)   ← the interactive TUI
    ├── observe/
    ├── registry/ (oci · skill · publish)
    └── cluster/ · status.rs                    ← single-file commands stay flat
```

- **One `Cli` + `Commands` enum in `main.rs`**; each group's `Args` in its `commands/` file.
  Doc comments become `--help` — write them for users.
- **Dispatch only in `main.rs`** → `chat::run(args)`. Logic lives in the command file.
- **`--json` serializes the same `dto` type the API returned**, so human and scripted output can't
  drift.
- **Exit codes:** `fn main() -> ExitCode`. `0` success · distinct non-zero for expected failures
  (not-found, auth-required) · `1` unexpected. `anyhow::Result<()>` alone collapses everything to `1`
  and breaks scripting.

---

## 4. The shared `dto/` crate

Request/response types used by **both** server and CLI live in one serde-only crate:

```
dto/src/                     ← depends on serde ONLY (no axum, sqlx, tokio, ureq)
├── lib.rs
├── agents.rs · chat.rs · obs.rs · catalog.rs · usage.rs · auth.rs · grants.rs · ...
```

**Why:** a single serde-only source of truth for the wire format keeps the server and CLI from
drifting apart on hand-rolled copies of the same structs. It stays serde-only so the sync CLI
(`ureq`) and async server (`axum`) can both depend on it without pulling in each other's stack.

**Where each type lives:**

| Type                                   | Home                              |
| -------------------------------------- | --------------------------------- |
| Request/response (crosses server↔CLI)  | `dto`                             |
| DB row struct (server-internal)        | the feature's `repo/<feature>.rs` |
| A library's own domain types           | that library's `models.rs`        |
| Cross-crate protocol types (A2A, etc.) | `types`                           |

Only **wire types** go to `dto/`. DB rows stay in `repo/`; internal domain types stay in a library's
`models.rs`.

---

## 5. Error handling

- **Libraries → `thiserror`.** One public enum per crate, model failure *kinds* so callers can
  `match` (never a single `Other(String)`), `#[from]` for wrapped sources, re-exported from `lib.rs`.
  `oci/src/error.rs` is the shape to copy.
- **Binaries (server **and** CLI) → `anyhow`.** `.context("deploying agent")` at each layer builds a
  readable failure trail. No error enums, no `error.rs` in binaries. In the server, map errors to
  HTTP status inline at the handler boundary.

Propagation: a library returns its `thiserror` enum → the binary lifts it with `?` + `.context()`.

---

## 6. Documentation

```
/
├── README.md               ← top-level: what Nasiko is, quickstart
├── CONTRIBUTING.md         ← contributors: build, test, PR flow
└── docs/                   ← design docs and standards
    ├── ARCHITECTURE-level design docs (A2A_REGISTRY_DESIGN.md, MCP_GATEWAY_DESIGN.md, …)
    ├── CLEAN_CODE_GUIDE.md ← code conventions and review standards
    ├── API_CONVENTIONS.md  ← wire-format standard for every HTTP endpoint
    └── ORGANIZATION.md     ← this file
```

- Docs that explain *why/where* age well; docs restating *what the code does* rot. Transient
  refactor notes and scratch TODOs are not docs.

---

## 7. Build / dev scripts

```
/
├── justfile                ← the front door: every common workflow is a recipe (keep ONE)
├── scripts/                ← shared bash/py, one task per file
└── <package>/deploy/       ← scripts that ship WITH a package live beside it
```

If a command appears twice in docs, make it a `just` recipe. Keep a single `justfile` at root (a
`justfile`/`Justfile` case-duplicate collides on case-insensitive filesystems).

---

## 8. Tests

```
<crate>/
├── src/foo.rs              ← unit tests inline: #[cfg(test)] mod tests { … }
└── tests/                  ← integration, one file per feature area
    ├── <area>.rs
    └── common/mod.rs       ← shared fixtures/helpers
```

- **Unit tests colocated** — read as executable docs, can test private items.
- **Integration tests in `tests/`**, one file per area, shared setup in `tests/common/`.
- **DB-touching server tests** `#[serial]` + single-threaded; tests needing external services
  (Ollama, LLM) `#[ignore]`d so default `cargo test` stays fast.
- **Name the behavior** — `rejects_expired_token`, not `test_1`.
- Repo implementations are tested against a real Postgres in `tests/`; the repo trait lets handler
  unit tests run against a mock repo.

---

## 9. Configuration / env management

```
SERVER   config.rs at src/ root  ← composes the shared config crate; validate() fail-fast at boot
LIBRARY  <no env access>         ← takes a Config/builder param from its caller
CLI      ~/.nasiko/config.json   ← loaded by the cli's config.rs
```

- Server env is read in one place (the `config` crate, surfaced through the server's `config.rs`)
  — never `std::env::var` in a handler.
- **Validate every field at startup**, fail-fast — don't surface misconfig lazily on first request.

---

## 10. Misc infra files

```
/
├── .github/workflows/        ← CI, one workflow file per pipeline
├── docker-compose.infra.yml  ← local infra (postgres/redis/minio)
└── server/Dockerfile         ← Dockerfile beside the code it packages
```

Dockerfiles beside their package (never a central `docker/` dump). CI/compose are workspace-wide,
so they sit at root.

---

## 11. Benchmarking

Bench in the **library** that owns the hot path — extract logic out of a binary first so it's both
benchable and reusable.

```
<lib>/
├── benches/<area>.rs   ← criterion, one file per area
└── Cargo.toml          ← [[bench]] name = "<area>", harness = false
```

Criterion as a workspace dev-dep, wired into a `just bench` recipe. Servers/CLIs get no benches
directly. Add only where there's a real hot path (router scoring, OCI blobs, secrets crypto).

---

## 12. Library examples

```
<lib>/examples/
├── <scenario>.rs   ← one self-contained use of the PUBLIC api, has fn main()
└── common/mod.rs   ← shared fixtures across examples
```

Each example demonstrates **one** public-API use, runs standalone
(`cargo run -p <crate> --example <name>`), and touches **only** the public API — if it needs
internals, the API has a gap; fix the API. Wire `cargo build --examples` into CI so a stale example
fails the build instead of drifting silently.
