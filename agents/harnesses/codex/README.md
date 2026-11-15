# codex harness image

Runs `codex-acp` (ACP over stdio) behind `harness-shim`, which serves it at
`ws://<host>:7000/acp`. Image: `nasiko/harness-codex:<tag>`.

Versions are build args (`CODEX_VERSION, CODEX_ACP_VERSION`), pinned in the Dockerfile; override them with `--build-arg`.
See [../README.md](../README.md) for the shim contract and build steps.
