target := "x86_64-unknown-linux-musl"
user := env("DOCKERHUB_USER", "nasiko")
docker := env("DOCKER", if path_exists("/usr/bin/podman") == "true" { "podman" } else { "docker" })

# Start backing infra (Postgres, Redis, S3)
infra:
    {{docker}} compose -f docker-compose.infra.yml up -d

# Stop backing infra (pass -v to remove volumes)
infra-down *args:
    {{docker}} compose -f docker-compose.infra.yml down {{args}}

# DESTRUCTIVE: wipe ALL local state (Postgres/S3 volumes + deployed agent containers), restart fresh infra
[confirm("This DELETES all local data: Postgres + S3 volumes and every nasiko-agent-* container. Continue? (y/N)")]
fresh:
    #!/usr/bin/env bash
    set -euo pipefail
    agents=$({{docker}} ps -aq --filter 'name=^/?nasiko-agent-')
    if [ -n "$agents" ]; then {{docker}} rm -f $agents; fi
    {{docker}} compose -f docker-compose.infra.yml down -v
    {{docker}} compose -f docker-compose.infra.yml up -d

# Show infra logs (-f to follow)
logs *args:
    {{docker}} compose -f docker-compose.infra.yml logs {{args}}

# Start infra + server (full local stack)
run-stack:
    #!/usr/bin/env bash
    set -euo pipefail
    {{docker}} compose -f docker-compose.infra.yml up -d
    set -a; source server/.env 2>/dev/null || source server/.env.example; set +a
    cargo run -p nasiko-server

# Run server (foreground)
run:
    #!/usr/bin/env bash
    set -euo pipefail
    set -a; source server/.env 2>/dev/null || source server/.env.example; set +a
    cargo run -p nasiko-server

# Run server only (alias)
run-server:
    #!/usr/bin/env bash
    set -euo pipefail
    set -a; source server/.env 2>/dev/null || source server/.env.example; set +a
    cargo run -p nasiko-server

# Run server with hot reload (requires cargo-watch: cargo install cargo-watch)
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    set -a; source server/.env 2>/dev/null || source server/.env.example; set +a
    cargo watch -x "run -p nasiko-server --bin nasiko-server"

# Release OSS control plane (server)
release-cp tag="latest":
    cargo zigbuild --release --target {{target}} -p nasiko-server
    {{docker}} buildx build --platform linux/amd64 -t {{user}}/nasiko-server:{{tag}} -f server/Dockerfile --push .

# ── Quality ───────────────────────────────────────────────────────────────────

# Type-check workspace
check:
    cargo check --workspace

# Lint workspace
clippy:
    cargo clippy --workspace

# ── Testing ───────────────────────────────────────────────────────────────────

# Phase I — unit tests, no infra required
test-unit:
    cargo test \
      -p nasiko \
      -p nasiko-auth \
      -p nasiko-compress \
      -p nasiko-secrets \
      -p nasiko-config \
      -p nasiko-utils \
      -p nasiko-types \
      -p nasiko-flow \
      -p nasiko-agent-proxy \
      -p nasiko-orchestrator \
      -p nasiko-runtime \
      -p nasiko-observability \
      -p nasiko-github \
      -p nasiko-llm-router \
      -p nasiko-harness-shim \
      -p nasiko-acp-client
    # `--lib` because these three keep their DB-backed suites in `tests/` (Phase II,
    # `test-crates` below) — only their hermetic lib tests belong in Phase I, which is why they
    # can't just join the list above. `nasiko-react-agent` has no `tests/` dir at all, so `--lib`
    # is everything it has.
    cargo test --lib -p nasiko-hitl -p nasiko-mcp-gateway -p nasiko-react-agent

# Build the harness shim (static musl) and the three platform harness images. The harness and ACP
# adapter versions are pinned in each image's Dockerfile (`ARG`s); the recipe passes the same pins.
# Needs the x86_64-unknown-linux-musl target and Docker. Usage: just build-harness-images [TAG] [REPO]
# (REPO is the registry namespace the images are tagged under, e.g. a Docker Hub user or org).
build-harness-images TAG="0.1.0" REPO="nasiko":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --release -p nasiko-harness-shim --target x86_64-unknown-linux-musl
    trap 'rm -f agents/harnesses/*/harness-shim agents/harnesses/*/harness-supervisor' EXIT
    ws=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
    declare -A args=(
      [opencode]="--build-arg OPENCODE_VERSION=1.19.5"
      [claude-code]="--build-arg CLAUDE_CODE_VERSION=2.1.312 --build-arg CLAUDE_CODE_ACP_VERSION=0.16.2"
      [codex]="--build-arg CODEX_VERSION=0.165.0 --build-arg CODEX_ACP_VERSION=0.16.0"
    )
    for h in opencode claude-code codex; do
      ctx=agents/harnesses/$h
      # Every image carries both binaries.
      for bin in harness-shim harness-supervisor; do
        cp "$ws/x86_64-unknown-linux-musl/release/$bin" "$ctx/$bin"
      done
      # The shim is a linux/amd64 binary and the sandbox workers are amd64, so the image is too,
      # whatever the build host.
      {{docker}} build --platform linux/amd64 ${args[$h]} -t {{REPO}}/harness-$h:{{TAG}} "$ctx"
    done

# End-to-end check of local A2A agents against a real CrewAI agent and a real LangGraph server:
# `nasiko agents discover`, then registration and LLM routing through a real nasiko-server
# (`nasiko connect`/`disconnect`). Needs python3 (3.10+) and `just infra`; the first run installs
# both frameworks. `just test-discovery --discovery-only` skips the server half.
test-discovery *args:
    scripts/a2a-discovery-e2e/run.sh {{args}}

# Phase II — library-crate integration tests that need Postgres (run `just infra` first).
# Serial for the same reason `test-server` is: they share one database, and `nasiko-hitl`'s own
# `tests/store.rs` truncates `hitl_requests` on entry. Kept a separate recipe from `test-server`
# so a failure names the layer it came from. Re-runs the lib tests `test-unit` already covered
# (~200 tests, well under a second) rather than glob-selecting every `--test` target by hand.
# `--include-ignored`: every test in `nasiko-hitl`'s own `tests/store.rs` is marked
# `#[ignore = "requires PostgreSQL"]` so a plain `cargo test` (no infra) doesn't fail — but that
# mark is static, not conditional on `DATABASE_URL`, so without this flag Postgres being up here
# never actually un-skips them (found in review: they were silently 0-run for every prior PR).
test-crates:
    cargo test -p nasiko-hitl -p nasiko-mcp-gateway -- --test-threads=1 --include-ignored

# Phase II — server integration tests (run `just infra` first)
test-server:
    cargo test -p nasiko-server -- --test-threads=1

# Phase II — single server test file  e.g. `just test-one auth_flow`
test-one name:
    cargo test -p nasiko-server --test {{name}} -- --test-threads=1

# All OSS tests: unit + library-crate and server integration
test: test-unit test-crates test-server
