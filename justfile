target := "x86_64-unknown-linux-musl"
user := env("DOCKERHUB_USER", "nasiko")
docker := env("DOCKER", if path_exists("/usr/bin/podman") == "true" { "podman" } else { "docker" })

# Start backing infra (Postgres, Redis, S3)
infra:
    {{docker}} compose -f ../docker-compose.infra.yml up -d

# Stop backing infra
infra-down:
    {{docker}} compose -f ../docker-compose.infra.yml down

# Show infra logs (-f to follow)
logs *args:
    {{docker}} compose -f ../docker-compose.infra.yml logs {{args}}

set dotenv-load

# Run server + gateway (foreground)
run:
    #!/usr/bin/env bash
    set -euo pipefail
    trap 'kill $(jobs -p) 2>/dev/null; wait' INT TERM
    cargo run -p nasiko-server & cargo run -p nasiko-gateway & wait

# Release OSS control plane (server + gateway)
release-cp tag="latest":
    cargo zigbuild --release --target {{target}} -p nasiko-server -p nasiko-gateway
    {{docker}} buildx build --platform linux/amd64 -t {{user}}/nasiko-server:{{tag}} -f oss/server/Dockerfile --push .
    {{docker}} buildx build --platform linux/amd64 -t {{user}}/nasiko-gateway:{{tag}} -f oss/gateway/Dockerfile --push .
