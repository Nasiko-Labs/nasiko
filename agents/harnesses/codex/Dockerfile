# Platform harness image: Codex behind the ACP-over-WebSocket shim.
# Build via `just build-harness-images`, which places prebuilt static
# `harness-shim` and `harness-supervisor` binaries in this build context.
FROM node:22-bookworm-slim

# curl exists only for the HEALTHCHECK below. Kubernetes ignores HEALTHCHECK; the provider
# probes /healthz itself. tini is the init process: the shim would otherwise be PID 1, and
# tokio only reaps its own child, so backgrounded grandchildren would become zombies.
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates curl openssh-client tini \
    && rm -rf /var/lib/apt/lists/*

# Pinned to concrete releases (checked on npm); `just build-harness-images` passes the same pins.
ARG CODEX_VERSION=0.165.0
ARG CODEX_ACP_VERSION=0.16.0
RUN npm install -g @openai/codex@${CODEX_VERSION} @zed-industries/codex-acp@${CODEX_ACP_VERSION}

COPY harness-shim /usr/local/bin/harness-shim
# One shim per session in a shared pod; a provider using it overrides the command (ENTRYPOINT stays the shim).
COPY harness-supervisor /usr/local/bin/harness-supervisor

ENV HOME=/workspace/home
# No update-check opt-out is needed: `codex-acp` drives Codex through its core library, and the
# upgrade notice belongs to the interactive `codex` binary, which never runs here. The provider
# and model arrive per session as the server's harness profile: a `config.toml` the shim writes
# to `~/.codex` before the harness starts (Codex derives its home from the session's HOME), so
# nothing is baked here.
WORKDIR /workspace
# Debian's nobody:nogroup, the uid the sandbox providers use; they own the /workspace mount.
USER 65534:65534

EXPOSE 7000
ENV HARNESS_CMD="codex-acp"
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s CMD curl -fsS http://127.0.0.1:7000/healthz || exit 1
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/harness-shim"]
