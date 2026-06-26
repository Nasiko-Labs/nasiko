target := "x86_64-unknown-linux-musl"
user := env("DOCKERHUB_USER", "nasiko")

# Start OSS dev environment (build from source + run full stack)
dev-up:
    cargo zigbuild --release --target {{target}} -p nasiko-server -p nasiko-gateway
    docker compose -f docker-compose.dev.yml up -d --build

# Stop OSS dev environment
dev-down:
    docker compose -f docker-compose.dev.yml down

# Release OSS control plane (server + gateway)
release-cp tag="latest":
    cargo zigbuild --release --target {{target}} -p nasiko-server -p nasiko-gateway
    docker buildx build --platform linux/amd64 -t {{user}}/nasiko-server:{{tag}} -f oss/server/Dockerfile --push .
    docker buildx build --platform linux/amd64 -t {{user}}/nasiko-gateway:{{tag}} -f oss/gateway/Dockerfile --push .
