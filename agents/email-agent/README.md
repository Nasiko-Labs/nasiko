# Email Agent

Nasiko A2A email assistant (HITL + optional MCP email tools).

## Local run (Docker)

```powershell
docker build -t nasiko-email-agent .
docker run --rm -p 8011:8000 `
  -e HOST_OVERRIDE=http://localhost:8011/ `
  -e OPENAI_API_KEY=<key> `
  -e OPENAI_BASE_URL=https://bedrock-mantle.us-east-1.api.aws/v1 `
  -e MODEL=openai.gpt-oss-20b `
  -e OTEL_SDK_DISABLED=true `
  nasiko-email-agent
```

- Health: `http://localhost:8011/health`
- Agent card: `http://localhost:8011/.well-known/agent-card.json`
- A2A JSON-RPC: `POST http://localhost:8011/` method `SendMessage`

Optional: `MCP_GATEWAY_URL` + `MCP_GATEWAY_TOKEN` for real email tools.

## Smoke tests (no MCP)

Send text `hitl input test`, `hitl auth test`, `hitl options test`, or `hitl multiselect test`.
