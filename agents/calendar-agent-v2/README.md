# Calendar Agent v2

Nasiko A2A calendar/scheduling assistant (HITL + optional MCP calendar tools).

## Local run (Docker)

```powershell
docker build -t nasiko-calendar-agent-v2 .
docker run --rm -p 8010:8000 `
  -e HOST_OVERRIDE=http://localhost:8010/ `
  -e OPENAI_API_KEY=<key> `
  -e OPENAI_BASE_URL=https://bedrock-mantle.us-east-1.api.aws/v1 `
  -e MODEL=openai.gpt-oss-20b `
  -e OTEL_SDK_DISABLED=true `
  nasiko-calendar-agent-v2
```

- Health: `http://localhost:8010/health`
- Agent card: `http://localhost:8010/.well-known/agent-card.json`
- A2A JSON-RPC: `POST http://localhost:8010/` method `SendMessage`

Optional: `MCP_GATEWAY_URL` + `MCP_GATEWAY_TOKEN` for real calendar tools.

## Smoke tests (no MCP)

Send text `hitl input test`, `hitl auth test`, `hitl options test`, or `hitl multiselect test`.
