# How to Run an Agent in Nasiko Locally

This doc covers everything needed to get Nasiko running locally, deploy an agent, and send queries through it — all using free APIs (no paid OpenAI quota needed).

---

## Prerequisites

- Docker + Docker Compose
- `gh` CLI (for GitHub operations)
- `uv` (Python package manager)

---

## 1. Environment Setup

Copy the example env file and fill in the required values:

```bash
cp .nasiko-local.env.example .nasiko-local.env
```

Required values in `.nasiko-local.env`:

```env
# LLM provider for the router (picks which agent to use)
ROUTER_LLM_PROVIDER=openrouter
ROUTER_LLM_MODEL=nvidia/nemotron-3-super-120b-a12b:free
OPENROUTER_API_KEY=sk-or-v1-...

# Embedding provider for vector store (only used when 15+ agents registered)
# Jina AI is free: https://jina.ai — 1M tokens/month, no model download
EMBEDDING_PROVIDER=jina
JINA_API_KEY=jina_...
JINA_EMBEDDING_MODEL=jina-embeddings-v3
```

Everything else in the file can stay as the defaults for local dev.

---

## 2. Start the Stack

**Important**: always pass `OPENAI_API_KEY` and `OPENROUTER_API_KEY` explicitly so Docker Compose doesn't pick up stale values from your shell environment.

```bash
OPENAI_API_KEY=$(grep ^OPENAI_API_KEY .nasiko-local.env | cut -d= -f2-) \
OPENROUTER_API_KEY=$(grep ^OPENROUTER_API_KEY .nasiko-local.env | cut -d= -f2-) \
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml up -d
```

Wait for everything to be healthy (~2-3 minutes):

```bash
docker compose -f docker-compose.local.yml ps
```

Services and their ports:
| Service | Port | Notes |
|---------|------|-------|
| Kong Gateway | 9100 | Main entry point for all API calls |
| Backend | 8000 | Direct access (skip Kong) |
| Web UI | 4000 | Frontend |
| Auth | 8082 | Auth service |
| Router | 8081 | Agent router direct port |
| Redis | 6379 | |
| MongoDB | 27017 | |

---

## 3. Get Login Credentials

The superuser is created automatically on first startup. Credentials are written to `orchestrator/superuser_credentials.json`:

```bash
cat orchestrator/superuser_credentials.json
```

You'll see:
```json
{
  "access_key": "NASK_...",
  "access_secret": "..."
}
```

### Log In

```bash
curl -s -X POST http://localhost:9100/auth/users/login \
  -H "Content-Type: application/json" \
  -d '{"access_key": "NASK_...", "access_secret": "..."}'
```

Save the `token` from the response — you'll need it for all subsequent API calls.

### Web UI

Open `http://localhost:4000` in your browser. Use the access key + secret to log in.

> **Note**: The web frontend is a compiled Flutter app with `https://nasiko.dev` hardcoded in the bundle. The docker-compose.local.yml patches this at container startup with `sed`, replacing it with `http://localhost:9100`. If the UI isn't connecting, check that the nasiko-web container started successfully.

---

## 4. Deploy an Agent

Agents live in the `agents/` directory. Each agent needs:
- `Dockerfile`
- `docker-compose.yml`
- `Agentcard.json` (A2A agent card with capabilities)
- `src/__main__.py` (entry point)

### Configure the Agent's LLM

For agents using OpenRouter, set the provider in `agents/<your-agent>/src/__main__.py` and in `agents/<your-agent>/docker-compose.yml`:

```yaml
# agents/<your-agent>/docker-compose.yml
environment:
  - OPENROUTER_API_KEY=${OPENROUTER_API_KEY:-}
  - OPENROUTER_MODEL=${OPENROUTER_MODEL:-nvidia/nemotron-3-super-120b-a12b:free}
```

The redis listener injects `OPENROUTER_API_KEY` automatically from its own environment (which gets it from `.nasiko-local.env`). You don't need to hardcode keys anywhere.

### Deploy via Redis Stream

The recommended way is to publish a deploy message directly to the Redis stream the listener watches:

```bash
docker exec redis redis-cli XADD orchestration:commands '*' \
  command deploy_agent \
  agent_name <your-agent-folder-name> \
  agent_path /app/agents/<your-agent-folder-name> \
  base_url http://nasiko-backend:8000 \
  upload_type directory
```

Watch the deployment:

```bash
docker logs nasiko-redis-listener -f
```

The listener will:
1. Copy the agent to a temp build dir
2. Inject Phoenix observability tracing
3. Build a Docker image (`local-agent-<name>:latest`)
4. Run the container on the `agents-net` network
5. Register the agent with Kong and the backend

After ~30-60 seconds (depending on dependencies to install), the agent container will be up:

```bash
docker ps | grep agent-
```

### Deploy via Web UI

Alternatively, use the Nasiko web UI at `http://localhost:4000` — navigate to Projects > Upload Agent and upload the agent directory as a zip.

---

## 5. Test End-to-End

### Get a Token

```bash
TOKEN=$(curl -s -X POST http://localhost:9100/auth/users/login \
  -H "Content-Type: application/json" \
  -d '{"access_key": "NASK_...", "access_secret": "..."}' \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['token'])")
```

### Send a Query Through the Router

The router endpoint is multipart/form-data (not JSON):

```bash
curl -s -X POST http://localhost:9100/router \
  -H "Authorization: Bearer $TOKEN" \
  -F "query=your query here" \
  -F "session_id=any-session-id"
```

The response is a stream of JSON lines. Each line has `is_int_response: true` for status updates and `is_int_response: false` for the final agent response.

Example with the a2a-translator agent:

```bash
curl -s -X POST http://localhost:9100/router \
  -H "Authorization: Bearer $TOKEN" \
  -F "query=Translate to French: Hello, how are you?" \
  -F "session_id=test-123"
```

---

## 6. LLM Provider Notes

### Router LLM (picks which agent to use)

Configured via `ROUTER_LLM_PROVIDER` and `ROUTER_LLM_MODEL` in `.nasiko-local.env`. Supported providers: `openai`, `openrouter`, `minimax`.

Recommended free setup:
```env
ROUTER_LLM_PROVIDER=openrouter
ROUTER_LLM_MODEL=nvidia/nemotron-3-super-120b-a12b:free
```

> The router uses LangChain's `.with_structured_output()` which sends `response_format: json_schema` to the provider. Not all free models support this — pick one that does. Nemotron works.

### Embeddings (for 15+ agents only)

The vector store is only created when there are 15 or more agents registered. Below that threshold the router skips it entirely and uses the LLM directly. So for small setups, you don't need an embedding provider at all.

If you do scale to 15+ agents, set:
```env
EMBEDDING_PROVIDER=jina
JINA_API_KEY=jina_...
```

OpenAI embeddings (`EMBEDDING_PROVIDER=openai`) are the default but require a paid key.

### Agent LLM

Each agent manages its own LLM client. The `a2a-translator` agent uses OpenRouter. The key is automatically passed from the redis listener's environment — just make sure `OPENROUTER_API_KEY` is in `.nasiko-local.env`.

---

## 7. Restarting Services

When you change `.nasiko-local.env`, always pass env vars explicitly to avoid shell overrides:

```bash
OPENAI_API_KEY=$(grep ^OPENAI_API_KEY .nasiko-local.env | cut -d= -f2-) \
OPENROUTER_API_KEY=$(grep ^OPENROUTER_API_KEY .nasiko-local.env | cut -d= -f2-) \
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml \
  up -d --force-recreate <service-name>
```

After restarting `nasiko-redis-listener`, always redeploy any running agents — the listener won't auto-reconnect to already-running containers and they may have stale env vars.

---

## 8. Troubleshooting

### Agent container crashes immediately

```bash
docker logs agent-<name>
```

Common causes:
- Missing `OPENROUTER_API_KEY` — check the listener has it: `docker exec nasiko-redis-listener env | grep OPENROUTER`
- Port conflict on 5000 — agents run internally on 5000, not exposed to host, accessed through Kong

### Router returns "No agents available"

The router caches agent registry responses. Restart the router after deploying a new agent:

```bash
OPENROUTER_API_KEY=$(grep ^OPENROUTER_API_KEY .nasiko-local.env | cut -d= -f2-) \
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml \
  up -d --force-recreate nasiko-router
```

### "Failed to create vector store" error

This means you have 15+ agents and the embedding provider isn't configured. Set `EMBEDDING_PROVIDER=jina` and `JINA_API_KEY` in `.nasiko-local.env` and restart the router. Below 15 agents this error never appears.

### Shell env overriding .nasiko-local.env

Docker Compose shell environment takes precedence over `--env-file`. If you have `OPENAI_API_KEY` or `OPENROUTER_API_KEY` set in your shell from a previous session, always prefix your compose commands:

```bash
OPENAI_API_KEY=... OPENROUTER_API_KEY=... docker compose --env-file .nasiko-local.env ...
```

### Web UI not connecting to backend

The Flutter bundle has `https://nasiko.dev` baked in at compile time. The nasiko-web container patches it at startup:

```bash
docker logs nasiko-web | head -5
```

You should see the sed patch running. If the patch failed, the UI will try to hit `nasiko.dev` instead of your local Kong.

### Kong returns 404 for /router

Check Kong routes are registered. The service registry registers routes on startup:

```bash
curl http://localhost:9101/routes | python3 -m json.tool | grep name
```

If `/router` is missing, restart `kong-service-registry`:

```bash
docker compose -f docker-compose.local.yml restart kong-service-registry
```

---

## Architecture Overview

```
Browser/curl
    |
    v
Kong Gateway (port 9100)
    |-- /auth/*      --> nasiko-auth-service
    |-- /api/v1/*    --> nasiko-backend
    |-- /router      --> nasiko-router
    |-- /agents/*    --> agent containers (registered dynamically)

nasiko-router
    |-- fetches agent list from nasiko-backend
    |-- uses LLM (OpenRouter) to pick the right agent
    |-- forwards request to selected agent via Kong

nasiko-redis-listener
    |-- watches Redis stream "orchestration:commands"
    |-- on deploy_agent: builds Docker image, runs container, registers with Kong

Agent containers
    |-- run on agents-net network
    |-- Kong discovers them via service registry
    |-- each agent has its own LLM client (OpenRouter, etc.)
```
