# syntax=docker/dockerfile:1
# Multi-stage build for nasiko-server.
# Build context must be the workspace root (oss/ in the public repo).
#
#   docker build -f server/Dockerfile -t nasiko-server .

# ── Stage 1: the web UI ─────────────────────────────────────────────────────
# The server embeds a BUILT React bundle (see server/build.rs), so something
# has to build it. Doing it here rather than committing ui/oss/app/dist to the repo
# keeps ~4 MB of generated output out of version control — one line changed in
# a component renames roughly two thirds of the bundle's files, because a
# content hash cascades through every chunk that imports it.
#
# It also means `docker compose up -d` needs nothing on the host but Docker:
# Node lives in this stage, not on the developer's machine.
FROM node:24-slim AS ui

WORKDIR /ui
# pnpm from Node's corepack, at the version ui/package.json names.
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
RUN corepack enable

# The lockfile alone decides what is downloaded, so this layer is cached until
# a dependency actually changes — editing a component fetches nothing.
# package.json too: corepack reads the pnpm version from it.
COPY ui/package.json ui/pnpm-workspace.yaml ui/pnpm-lock.yaml ./
RUN pnpm fetch

# Then the whole tree, installed offline from what was fetched. All of ui/, not
# the manifests alone: a frozen install needs every workspace member's
# package.json, and the members differ between this repo and the public one.
COPY ui/ ./
RUN pnpm install --offline --frozen-lockfile
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
COPY --from=ui /ui/oss/app/dist ./ui/oss/app/dist

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
