# syntax=docker/dockerfile:1
# Multi-stage build for nasiko-server.
# Build context must be the workspace root (oss/ in the public repo).
#
#   docker build -f server/Dockerfile -t nasiko-server .

# ── Stage 1: the web UI ─────────────────────────────────────────────────────
# The server embeds a BUILT React bundle (see server/build.rs), so something
# has to build it. Doing it here rather than committing ui/oss/dist to the repo
# keeps ~4 MB of generated output out of version control — one line changed in
# a component renames roughly two thirds of the bundle's files, because a
# content hash cascades through every chunk that imports it.
#
# It also means `docker compose up -d` needs nothing on the host but Docker:
# Node lives in this stage, not on the developer's machine.
FROM node:24-slim AS ui

WORKDIR /ui

# Manifests first, so the install layer is cached until a dependency actually
# changes — editing a component does not re-run npm ci.
#
# `common/package.json` as well as the root one: ui/ is an npm workspace whose
# single member is `common`, and that is where the runtime dependencies are
# declared. Installing from the root manifest alone succeeds and then fails the
# build much later with "Can't resolve 'tw-animate-css'", because npm had no
# workspace member to read those dependencies from.
COPY ui/package.json ui/package-lock.json ./
COPY ui/common/package.json ./common/package.json
RUN --mount=type=cache,target=/root/.npm npm ci

COPY ui/ ./
# `oss` explicitly: in the private repo this tree also holds the enterprise
# edition, and an unqualified build would produce both. The public repo has
# only this one, and scripts/editions.ts finds editions by directory shape, so
# the same command is correct in both.
RUN node scripts/app.ts build oss

# ── Stage 2: build ──────────────────────────────────────────────────────────
FROM rust:latest AS builder

WORKDIR /build
COPY . .

# rust-embed resolves this at compile time and hard-errors if it is missing, so
# it has to land before cargo runs. After `COPY . .`, which would otherwise
# shadow it with whatever (if anything) the context carried.
COPY --from=ui /ui/oss/dist ./ui/oss/dist

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release -p nasiko-server && \
    cp target/release/nasiko-server /nasiko-server

# ── Stage 3: runtime ────────────────────────────────────────────────────────
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /nasiko-server /usr/local/bin/nasiko-server

EXPOSE 8080
ENTRYPOINT ["nasiko-server"]
