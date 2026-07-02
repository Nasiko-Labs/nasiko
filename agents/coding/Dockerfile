# The coding agent's tools shell out to build/test toolchains, so unlike the other agents
# (which are FROM scratch) the runtime image must carry a shell and a toolchain. This image
# bundles the Rust toolchain; build per-language variants (-python, -node, -go) from the same
# binary with a different base when deploying for those ecosystems.
FROM rust:1-slim

# git is needed for repo operations; ripgrep speeds up search_code (falls back to grep if absent).
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ripgrep ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY target/x86_64-unknown-linux-musl/release/nasiko-coding-agent /usr/local/bin/coding-agent

ENV WORKSPACE_DIR=/workspace
ENV PORT=8000
RUN mkdir -p /workspace
WORKDIR /workspace

EXPOSE 8000
ENTRYPOINT ["/usr/local/bin/coding-agent"]
