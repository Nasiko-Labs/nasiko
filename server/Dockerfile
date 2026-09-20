# syntax=docker/dockerfile:1
# Multi-stage build for nasiko-server.
# Build context must be the workspace root (oss/ in the public repo).
#
#   docker build -f server/Dockerfile -t nasiko-server .

# ── Stage 1: build ──────────────────────────────────────────────────────────
FROM rust:latest AS builder

WORKDIR /build
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release -p nasiko-server && \
    cp target/release/nasiko-server /nasiko-server

# ── Stage 2: runtime ────────────────────────────────────────────────────────
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /nasiko-server /usr/local/bin/nasiko-server

EXPOSE 8080
ENTRYPOINT ["nasiko-server"]