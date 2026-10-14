# syntax=docker/dockerfile:1
# Multi-stage build for nasiko-server.
# Build context must be the workspace root (oss/ in the public repo).
#
#   docker build -f server/Dockerfile -t nasiko-server .
#
# Stage 1 builds the React SPA, stage 2 compiles it into the binary, stage 3 runs it. Node is
# build tooling only: nothing of it reaches the runtime image, and `docker compose up -d` needs
# nothing on the host but Docker.

# ── Stage 1: the web UI (OSS edition, release, live API) ────────────────────────────────────
# The server embeds a BUILT bundle (see server/build.rs), and none is committed: one line changed
# in a component renames most of the bundle's files, because a content hash cascades through
# every chunk that imports it.
#
# Two layouts, detected rather than assumed, because this file is published:
#   react/ (pnpm workspace) — the UI. The private repo, and the public one once React
#                             publication opens.
#   ui/common/ (npm)        — the frozen legacy UI the public repo keeps until then
#                             (docs/UI_SYNC.md §4). Delete this branch when the gate opens.
# The whole context is copied (not one tree) because COPY cannot name an optional directory;
# the pnpm store cache keeps the cost to the build itself.
FROM node:24-slim AS ui
WORKDIR /ctx
# Telemetry off for every tool in the build; the workspace's preinstall check requires it.
ENV DO_NOT_TRACK=1 OPENUI_TELEMETRY_DISABLED=1 STORYBOOK_DISABLE_TELEMETRY=1 TURBO_TELEMETRY_DISABLED=1 \
    COREPACK_ENABLE_DOWNLOAD_PROMPT=0 CI=1
RUN corepack enable
COPY . .
# React: a release build against the live API — no test build, no harness, no mock worker.
# build.rs re-checks the result and fails the Rust build on anything else.
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    --mount=type=cache,target=/root/.npm \
    set -eu; mkdir -p /out; \
    if [ -f react/pnpm-workspace.yaml ]; then \
        cd react && pnpm install --frozen-lockfile && \
        NASIKO_TEST_BUILD=0 NASIKO_HARNESS=0 NASIKO_DEPLOYMENT=workspace VITE_NASIKO_API_MODE=live \
            pnpm --filter @nasiko/web build && \
        cp -R oss/app/dist/. /out/; \
    elif [ -d ui/common ]; then \
        cd ui && npm ci && node scripts/app.ts build oss && cp -R oss/dist/. /out/; \
    else \
        echo "no UI tree in the build context (react/ or ui/)" >&2; exit 1; \
    fi

# ── Stage 2: build ──────────────────────────────────────────────────────────────────────────
FROM rust:1.95.0 AS builder
WORKDIR /build
COPY . .
COPY --from=ui /out /ui-dist
# A React bundle is selected explicitly (build.rs validates it); a legacy bundle goes where the
# legacy branch of build.rs looks for it.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    if [ -f /ui-dist/nasiko-spa-edition ]; then export NASIKO_REACT_DIST_OSS=/ui-dist; \
    else mkdir -p ui/oss/dist && cp -R /ui-dist/. ui/oss/dist/; fi && \
    cargo build --locked --release -p nasiko-server && \
    cp target/release/nasiko-server /nasiko-server

# ── Stage 3: runtime ────────────────────────────────────────────────────────────────────────
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /nasiko-server /usr/local/bin/nasiko-server
EXPOSE 8080
ENTRYPOINT ["nasiko-server"]
