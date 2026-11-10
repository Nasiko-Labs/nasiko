# claude-code harness image

Runs `claude-code-acp` (ACP over stdio) behind `harness-shim`, which serves it at
`ws://<host>:7000/acp`. Image: `nasiko/harness-claude-code:<tag>`.

Versions are build args (`CLAUDE_CODE_VERSION, CLAUDE_CODE_ACP_VERSION`), default `latest`; the release process pins them.
See [../README.md](../README.md) for the shim contract and build steps.
