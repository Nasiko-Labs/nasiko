# Good First Issue Guide

This guide explains how a new contributor can find, claim, implement, test,
and submit a beginner-friendly contribution to Nasiko.

## 1. Find an issue

* Browse open issues at `https://github.com/Nasiko-Labs/nasiko/issues`.
* Look for issues labeled `good first issue` or `docs` / `community`.
* Read the full issue: scope, starting files, dependencies, and definition
  of done. Check `README.md`, `CONTRIBUTING.md`, and the relevant `docs/`
  page before asking questions.

Good starting points in this repository:

* Docs and community-health improvements.
* Small, isolated fixes with an existing test file nearby.

## 2. Claim the issue

* One contributor owns one primary issue at a time.
* Comment on the issue with a short implementation plan and your expected
  date for the first update.
* Wait for maintainer assignment before doing substantial work. Provider
  issues and gated roadmap work must be explicitly approved.
* If there is no visible update for five days, maintainers may reopen the
  issue for another contributor.

Use GitHub Discussions, issue threads, and the community Discord linked in
`README.md` (`https://discord.com/invite/HmnfkTfjFv`) to ask questions and
share progress. Search existing issues and discussions first.

## 3. Set up the repository

Prerequisites: Rust (stable via `rustup`), Docker or Podman, and `just`
(`cargo install just`). See `CONTRIBUTING.md` for details.

```sh
git clone https://github.com/Nasiko-Labs/nasiko.git
cd nasiko
just infra
cp server/.env.example server/.env
# Edit server/.env: set OPENAI_API_KEY at minimum.
just run
```

Install the CLI from the checkout:

```sh
cargo build --release -p nasiko
```

For Docker-only verification, `docker compose up -d` starts Postgres, Redis,
RustFS (S3), the OTel stack, and `nasiko-server` on `http://localhost:8080`.

## 4. Implement the change

* Create a focused branch: `git checkout -b fix/<issue-number>-short-name`.
* Keep the change to the claimed scope; put unrelated cleanup in a separate PR.
* Follow `docs/CLEAN_CODE_GUIDE.md`, `docs/ORGANIZATION.md`, and
  `docs/API_CONVENTIONS.md`.
* Do not commit secrets. `.env` files are gitignored.
* For agents, follow `docs/A2A_PROTOCOL.md` (A2A v1.0, `A2A-Version: 1.0`).

## 5. Test

Run formatting, lints, and the appropriate test phase from `CONTRIBUTING.md`
and `justfile`:

```sh
cargo fmt
cargo check --workspace
cargo clippy --workspace
just test-unit
```

For server changes that need backing services:

```sh
just infra
just test-server
# Or a single file: just test-one <name>
```

CI fails on warnings. Keep unit tests hermetic (no network, DB, or Docker).

## 6. Submit the pull request

* Ensure `cargo fmt`, `cargo check --workspace`, and `cargo clippy` pass
  with zero warnings.
* Push your branch and open a PR against `main` that references the issue
  (e.g. `Fixes #123`).
* Follow `.github/pull_request_template.md`: summary, related issue, testing
  performed, documentation updates, breaking changes, and checklist.
* Include tests for new behavior, update docs when behavior or setup changes,
  and attach logs, command output, or screenshots that prove the definition
  of done is met.
* Respond to review, keep your branch current, and help maintainers verify
  the result. Do not include live credentials; use deterministic fixtures or
  documented local services.
