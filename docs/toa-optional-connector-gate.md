# Optional TOA verify before connector enable / promote

Example only. Copy into an ops or CI workflow that registers or enables custom MCP connectors in Nasiko.

Nasiko's MCP Gateway handles probe, SSRF checks, credentials, and per-agent permissions.
[TOA](https://github.com/Carmel-Labs-Inc/toa) (`toa/0.1`) is adjacent delivery evidence: a signed JSON you verify offline when `toa.json` is present.

TOA does not replace connector probe, OAuth, or ACLs. No AgentStatus account is required to verify.

```yaml
# .github/workflows/nasiko-connector-toa.yml
name: Optional TOA before connector promote
on:
  workflow_dispatch:
  pull_request:
    paths:
      - "connectors/**"
      - "toa.json"

jobs:
  toa:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      # Your existing probe / register / policy steps go here.

      - name: Verify tool delivery attestation
        if: hashFiles('toa.json') != ''
        run: |
          pip install "git+https://github.com/Carmel-Labs-Inc/toa.git@5a1bf1cf6a15a4864ea809fe7b2a073f2cef4e22#subdirectory=python"
          toa-verify toa.json --require-emitter agentstatus --require-layer functional=pass --max-age 7d
```

Design context: [MCP_GATEWAY_DESIGN.md](./MCP_GATEWAY_DESIGN.md) §15a.
