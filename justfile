target := "x86_64-unknown-linux-musl"
user := env("DOCKERHUB_USER", "nasiko")
docker := env("DOCKER", if path_exists("/usr/bin/podman") == "true" { "podman" } else { "docker" })

# Start backing infra (Postgres, Redis, S3)
infra:
    {{docker}} compose -f docker-compose.infra.yml up -d

# Stop backing infra
infra-down:
    {{docker}} compose -f docker-compose.infra.yml down

# Show infra logs (-f to follow)
logs *args:
    {{docker}} compose -f docker-compose.infra.yml logs {{args}}

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

# Release OSS control plane (server)
release-cp tag="latest":
    cargo zigbuild --release --target {{target}} -p nasiko-server
    {{docker}} buildx build --platform linux/amd64 -t {{user}}/nasiko-server:{{tag}} -f oss/server/Dockerfile --push .

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
      -p nasiko-auth \
      -p nasiko-secrets \
      -p nasiko-config \
      -p nasiko-utils \
      -p nasiko-types \
      -p nasiko-flow \
      -p nasiko-agent-proxy \
      -p nasiko-orchestrator \
      -p nasiko-runtime \
      -p nasiko-observability \
      -p nasiko-github

# Phase II — server integration tests (run `just infra` first)
test-server:
    cargo test -p nasiko-server -- --test-threads=1

# Phase II — single server test file  e.g. `just test-one auth_flow`
test-one name:
    cargo test -p nasiko-server --test {{name}} -- --test-threads=1

# All OSS tests: unit + server integration
test: test-unit test-server
