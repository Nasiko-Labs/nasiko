# Platform harness image: OpenCode behind the ACP-over-WebSocket shim.
# Build via `just build-harness-images`, which places prebuilt static
# `harness-shim` and `harness-supervisor` binaries in this build context.
FROM node:22-bookworm-slim

# curl exists only for the HEALTHCHECK below. Kubernetes ignores HEALTHCHECK; the provider
# probes /healthz itself. tini is the init process: the shim would otherwise be PID 1, and
# tokio only reaps its own child, so backgrounded grandchildren would become zombies.
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates curl openssh-client tini \
    && rm -rf /var/lib/apt/lists/*

# Pinned to a concrete release (checked on npm); `just build-harness-images` passes the same pin.
ARG OPENCODE_VERSION=1.19.5
RUN npm install -g opencode-ai@${OPENCODE_VERSION}

COPY harness-shim /usr/local/bin/harness-shim
# One shim per session in a shared pod; a provider using it overrides the command (ENTRYPOINT stays the shim).
COPY harness-supervisor /usr/local/bin/harness-supervisor

ENV HOME=/workspace/home
# The image is immutable and the sandbox's egress is an allowlist: no self-update, and no
# models.dev catalog fetch (the pinned release bundles a snapshot). Both flags are read by the
# pinned binary. The model, provider and permissions arrive per session as
# OPENCODE_CONFIG_CONTENT (the server's harness profile), never baked here.
ENV OPENCODE_DISABLE_AUTOUPDATE=1 OPENCODE_DISABLE_MODELS_FETCH=1
WORKDIR /workspace
# Debian's nobody:nogroup, the uid the sandbox providers use; they own the /workspace mount.
USER 65534:65534

EXPOSE 7000
ENV HARNESS_CMD="opencode acp"
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s CMD curl -fsS http://127.0.0.1:7000/healthz || exit 1
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/harness-shim"]
