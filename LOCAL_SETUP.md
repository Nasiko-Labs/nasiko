# LOCAL_SETUP.md — Nasiko Hackathon Setup

> **Status**: Repo already cloned. `.nasiko-local.env` is pre-created at the repo root.  
> You only need to paste one API key and run one command.

---

## Quickstart (Hackathon Mode)

### Prerequisites

| Tool | Minimum Version | Notes |
|------|----------------|-------|
| Docker Desktop | 4.x | Includes Docker Compose v2 |
| Docker Compose | v2 (bundled) | Use `docker compose` (no hyphen) |
| Git | any | Already used — repo is cloned |

> Python, Node, pnpm, uv are **not needed** for the Docker path. Skip them.

---

### Step 1 — Paste your LLM provider key

Open `.nasiko-local.env` in the repo root and find the OpenRouter block:

```env
# PASTE YOUR OPENROUTER KEY HERE:
OPENROUTER_API_KEY=sk-or-your-openrouter-api-key-here
ROUTER_LLM_PROVIDER=openrouter
ROUTER_LLM_MODEL=nvidia/nemotron-3-super-120b-a12b:free
```

Replace `sk-or-your-openrouter-api-key-here` with your real key.

**Get a free OpenRouter key** (takes ~2 minutes):  
1. Go to https://openrouter.ai → Sign Up  
2. Dashboard → API Keys → Create Key  
3. Paste the key (starts with `sk-or-v1-...`) into the line above

> Alternatively, if you have a paid OpenAI key, comment out the OpenRouter block and uncomment the OpenAI block (`OPTION B`) in `.nasiko-local.env`.

---

### Step 2 — Start everything (one command)

**Windows PowerShell:**
```powershell
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml up -d
```

**macOS / Linux / Git Bash:**
```bash
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml up -d
```

Docker will build several images from source on the first run. Expect **5–15 minutes** the first time. Subsequent starts take ~2–3 minutes.

---

### Step 3 — Wait for services to be healthy

```bash
# Repeat this until all services show "running" or "healthy"
docker compose -f docker-compose.local.yml ps
```

Expected services and their states:

| Container | Expected State |
|-----------|---------------|
| mongodb | healthy |
| redis | healthy |
| kong-database | healthy |
| kong-migrations | exited (0) — normal, it's a one-shot job |
| kong-gateway | healthy |
| nasiko-backend | healthy |
| nasiko-auth-service | healthy |
| nasiko-router | healthy |
| nasiko-web | running |
| kong-service-registry | healthy |
| chat-history-service | healthy |
| phoenix-observability | healthy |
| nasiko-superuser-init | exited (0) — normal, one-shot job |
| nasiko-redis-listener | running |

---

### Step 4 — Get your login credentials

```bash
# Linux / macOS / Git Bash
cat orchestrator/superuser_credentials.json

# Windows PowerShell
Get-Content orchestrator\superuser_credentials.json
```

You'll see:
```json
{
  "access_key": "NASK_...",
  "access_secret": "..."
}
```

---

### Step 5 — Open the UI and log in

Navigate to **http://localhost:9100/app/**  
Enter the `access_key` and `access_secret` from step 4 → Sign In.

---

### Health-check URLs

| URL | What it checks |
|-----|---------------|
| http://localhost:9100/app/ | **Main web UI** (entry point) |
| http://localhost:8000/api/v1/healthcheck | Backend API |
| http://localhost:9100/health | Kong gateway |
| http://localhost:8081/health | Router service |
| http://localhost:8082/health | Auth service |
| http://localhost:6006 | Phoenix observability dashboard |
| http://localhost:9102 | Kong Manager (gateway config UI) |
| http://localhost:8000/docs | REST API docs (Swagger) |

---

## Configuring OpenAI / Providers

### All LLM provider env vars (defined in this repo)

All of these live in `.nasiko-local.env` at the repo root.

| Variable | Provider | Required to **boot**? | Required to **route queries**? |
|----------|----------|:---------------------:|:------------------------------:|
| `OPENAI_API_KEY` | OpenAI | No | Only if `ROUTER_LLM_PROVIDER=openai` |
| `OPENROUTER_API_KEY` | OpenRouter | No | Only if `ROUTER_LLM_PROVIDER=openrouter` |
| `MINIMAX_API_KEY` | MiniMax | No | Only if `ROUTER_LLM_PROVIDER=minimax` |
| `MINIMAX_BASE_URL` | MiniMax | No | Only with MiniMax provider |
| `ROUTER_LLM_PROVIDER` | Router config | No (default: `openai`) | Yes — picks which LLM the router uses |
| `ROUTER_LLM_MODEL` | Router config | No (default: `gpt-4o-mini`) | Yes — picks the specific model |
| `JINA_API_KEY` | Jina embeddings | No | Only with 15+ agents registered |
| `JINA_EMBEDDING_MODEL` | Jina config | No | Only with 15+ agents registered |
| `EMBEDDING_PROVIDER` | Router config | No (default: `openai`) | Only with 15+ agents registered |

### Where to paste your OpenAI key

In `.nasiko-local.env`, find:
```env
OPENAI_API_KEY=sk-placeholder-not-needed-for-openrouter
```
Replace the placeholder with your real key. Then also set:
```env
ROUTER_LLM_PROVIDER=openai
ROUTER_LLM_MODEL=gpt-4o-mini
```
And comment out the OpenRouter lines.

### Does Nasiko need a real key just to boot?

**No.** All services start and the UI loads with placeholder values.  
A real LLM provider key is only needed when you send a query through the router.

### Cheapest working setup (zero cost)

1. Sign up free at https://openrouter.ai  
2. Create an API key  
3. Set in `.nasiko-local.env`:

```env
OPENROUTER_API_KEY=sk-or-v1-...
ROUTER_LLM_PROVIDER=openrouter
ROUTER_LLM_MODEL=nvidia/nemotron-3-super-120b-a12b:free
```

The Nemotron model is free and supports the structured JSON output format the router requires.

**Embeddings** (for the vector store) are only created when **15 or more agents** are registered. Below that, the router uses the LLM directly — no embedding key needed.

---

## Useful Commands

```bash
# View all logs in real time
docker compose -f docker-compose.local.yml logs -f

# View logs for a specific service
docker compose -f docker-compose.local.yml logs -f nasiko-backend
docker compose -f docker-compose.local.yml logs -f nasiko-redis-listener

# Stop all services (keeps data volumes)
docker compose -f docker-compose.local.yml down

# Full clean restart — deletes all data (MongoDB, Redis, Kong DB)
docker compose -f docker-compose.local.yml down -v
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml up -d

# Restart a single service after changing its env vars
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml up -d --force-recreate nasiko-router

# Deploy the bundled sample translator agent (once stack is up)
docker exec redis redis-cli XADD orchestration:commands '*' \
  command deploy_agent \
  agent_name a2a-translator \
  agent_path /app/agents/a2a-translator \
  base_url http://nasiko-backend:8000 \
  upload_type directory
```

---

## Troubleshooting

### Services not healthy after 10 minutes
```bash
docker compose -f docker-compose.local.yml logs nasiko-backend
docker compose -f docker-compose.local.yml logs nasiko-redis-listener
```

### Web UI loads blank or can't reach backend
The frontend is a compiled Flutter app with the production URL (`https://nasiko.dev`) baked in at compile time. The `nasiko-web` container patches this at startup with `sed`. Check if the patch ran:
```bash
docker logs nasiko-web | head -10
```

### Router returns "No agents available"
No agents are deployed yet. Use the UI: **Add Agent → Upload ZIP → select `agents/a2a-translator.zip`**, or run the redis command above. After deploying, restart the router:
```bash
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml up -d --force-recreate nasiko-router
```

### Kong returns 404 for `/router`
The service registry registers Kong routes on startup. Restart it:
```bash
docker compose -f docker-compose.local.yml restart kong-service-registry
```

### Port conflict (address already in use)
Change the relevant `NASIKO_PORT_*` variable in `.nasiko-local.env` and restart.

### `OPENAI_API_KEY` in shell overrides `.nasiko-local.env`
Docker Compose prioritizes shell environment over `--env-file`. If you have an old key exported in your shell:
```bash
# Linux/macOS: unset before running
unset OPENAI_API_KEY && docker compose --env-file .nasiko-local.env -f docker-compose.local.yml up -d
```

---

## Architecture at a glance

```
Browser
  └── Kong Gateway (port 9100)
        ├── /app/      → nasiko-web (Flutter UI)
        ├── /api/v1/   → nasiko-backend (FastAPI)
        ├── /auth/     → nasiko-auth-service (JWT / OAuth)
        ├── /router    → nasiko-router (LLM picks best agent)
        └── /agents/*  → agent containers (deployed dynamically)

nasiko-redis-listener
  └── watches Redis stream "orchestration:commands"
      on deploy_agent: builds Docker image, starts container, registers with Kong

Agent containers
  └── run on agents-net network, accessed through Kong
```

Services that build from source (expect build time on first `up`):
- `nasiko-backend` — FastAPI app in `app/`
- `nasiko-router` — LangChain router in `agent-gateway/router/`
- `chat-history-service` — service in `agent-gateway/chat-history-service/`
- `kong-service-registry` — service in `agent-gateway/registry/`
- `nasiko-redis-listener` — worker in `orchestrator/`
- `nasiko-superuser-init` — one-shot init job

Services pulled from Docker Hub (fast):
- `mongodb`, `redis`, `kong-gateway`, `kong-database` (postgres), `phoenix-observability`, `nasiko-auth-service`, `nasiko-web`
