# claude-code harness image

Runs `claude-code-acp` (ACP over stdio) behind `harness-shim`, which serves it at
`ws://<host>:7000/acp`. Image: `nasiko/harness-claude-code:<tag>`.

Versions are build args (`CLAUDE_CODE_VERSION`, `CLAUDE_CODE_ACP_VERSION`), pinned in the Dockerfile and in
`just build-harness-images` (`2.1.312` and `0.16.2`). Like the other harness images it also carries `harness-supervisor`.
See [../README.md](../README.md) for the shim contract and build steps.
