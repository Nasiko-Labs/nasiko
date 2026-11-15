# opencode harness image

Runs `opencode acp` (ACP over stdio) behind `harness-shim`, which serves it at
`ws://<host>:7000/acp`. Image: `nasiko/harness-opencode:<tag>`.

Versions are build args (`OPENCODE_VERSION`), pinned in the Dockerfile; override them with `--build-arg`.
See [../README.md](../README.md) for the shim contract and build steps.
