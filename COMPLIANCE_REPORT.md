# Nasiko MCP Calculator Agent — Full Chat Context

> **Session Date**: April 19, 2026  
> **Constraint**: Only `my-agent/` directory is editable. All other Nasiko platform code is **read-only**.  
> **Repo**: https://github.com/lazyKid64/nasikoHack (do NOT modify)

---

## Project Structure (my-agent/ only)

```
my-agent/
├── CHAT_CONTEXT.md              ← This file (session context for AI assistants)
├── README.md                    ← Project overview
├── VIDEO_DEMO_GUIDE.md          ← Demo recording guide
├── COMPLIANCE_REPORT.md         ← A2A compliance report
├── PROJECT_CONTEXT.md           ← Full project context
├── Dockerfile                   ← Container for the MCP bridge
├── Makefile                     ← Build/test shortcuts
├── pyproject.toml               ← Python project config
├── conftest.py                  ← Pytest config
│
├── examples/
│   └── mcp-calculator-server-v2/   ← ★ THE AGENT SOURCE (deployed to Nasiko)
│       ├── Agentcard.json           ← Agent metadata for registry
│       ├── Dockerfile               ← Container build file
│       ├── docker-compose.yml       ← Local dev compose
│       └── src/
│           ├── main.py              ← Main server (CMD target)
│           └── __main__.py          ← Copy of main.py (for injection)
│
├── scripts/                     ← ★ ALL OPERATIONAL SCRIPTS
│   ├── build.py                 ← Build dist/mcpcalc.zip from source
│   ├── deploy.py                ← Full pipeline: build → extract → Redis deploy
│   ├── test.py                  ← E2E test through Kong gateway
│   ├── chat.py                  ← Interactive terminal chat
│   └── cleanup.py               ← Remove files leaked outside my-agent/
│
├── dist/                        ← ★ BUILD OUTPUT
│   └── mcpcalc.zip              ← Ready-to-upload zip
│
├── tests/                       ← Unit tests
├── docs/                        ← Documentation
├── demo/                        ← Demo assets
└── nasiko/                      ← MCP bridge module
```

### Key Rule: Nothing Outside my-agent/
- All source code lives in `my-agent/examples/mcp-calculator-server-v2/`
- All scripts live in `my-agent/scripts/`
- Build output goes to `my-agent/dist/`
- The `deploy.py` script writes to `nasiko/agents/mcpcalc/` at **runtime only** (required by the platform pipeline) — this is NOT committed to git

---

## Terminal Commands Quick Reference

All commands run from `my-agent/` directory:

```powershell
# Build the zip
py -3 scripts/build.py

# Full deploy (build + extract + trigger Redis)
py -3 scripts/deploy.py

# Run E2E tests
py -3 scripts/test.py

# Interactive chat
py -3 scripts/chat.py

# Clean up files leaked outside my-agent/
py -3 scripts/cleanup.py

# Watch agent logs
docker logs agent-mcpcalc -f

# Restart crashed agent
docker start agent-mcpcalc

# Watch deployment progress
docker logs nasiko-redis-listener -f

# View Phoenix traces
# Open http://localhost:6006 → Tracing → mcp-calculator
```

---

## Platform Architecture (Read-Only)

```
┌─────────────┐    ┌──────────────┐    ┌─────────────────┐    ┌──────────────┐
│  Flutter UI  │───▶│  Kong Gateway │───▶│  Agent Container │───▶│   Phoenix    │
│ :9100/app    │    │  :9100       │    │  :5000 internal  │    │  :6006       │
└─────────────┘    └──────────────┘    └─────────────────┘    └──────────────┘
       │                                        ▲
       ▼                                        │
┌─────────────┐    ┌──────────────┐    ┌────────┴────────┐
│  Backend API │───▶│    Redis     │───▶│  Redis Listener  │
│  :8000       │    │  :6379      │    │  (Orchestrator)  │
└─────────────┘    └──────────────┘    └─────────────────┘
```

### Key Services
| Service | Container Name | Port | Role |
|---------|---------------|------|------|
| Flutter Web UI | (via Kong) | 9100 | User-facing web app |
| Kong Gateway | kong-gateway | 9100 (proxy), 9101 (admin) | Routes `/agents/agent-*` to containers |
| Backend API | nasiko-backend | 8000 | Upload API, registry, auth |
| Redis | nasiko-redis | 6379 | Message stream for deployment commands |
| Redis Listener | nasiko-redis-listener | — | Orchestrates builds, deploys containers |
| Phoenix | phoenix-observability | 6006 (UI + OTLP) | Trace collection & visualization |
| Chat History | nasiko-chat-history | — | Stores chat messages |
| Router | nasiko-router | — | Routes chat messages to agents |
| Auth | nasiko-auth-service | 8001 | Agent permissions |
| MongoDB | nasiko-mongo | 27017 | Registry, uploads, sessions |

### Superuser Credentials
```
Access Key:    NASK_GTNrAVE8ZbTDO1-4w4Tx5w
Access Secret: SZePcCtMM0c_2msnfIfFXjwl4KfvAEEbi3f5N2pBpO0
User ID:       1e05d931-2958-46da-85be-0f7af203f1a7
Username:      superuser-username
```

---

## Bugs Found & Fixed

### Bug 1: A2A Response Format — Blank UI Responses
**Cause**: Platform requires `result.kind = "message"` and `parts[].kind = "text"`. Agent used wrong keys.  
**Fix**: Updated response format in `main.py` and `__main__.py`.

### Bug 2: MongoDB DuplicateKeyError — Agent Not Registering
**Cause**: Agent name `"MCP Calculator Server"` conflicted with existing entries.  
**Fix**: Changed to `"MCP Calculator v2"` in `Agentcard.json`, `main.py`, `__main__.py`.

### Bug 3: Container Name Conflict — Deployment Failing
**Cause**: Old stopped containers blocked `docker run --name agent-mcpcalc`.  
**Fix**: Stop + remove old containers before deploy. Deploy script handles this.

### Bug 4: Phoenix Tracing Wrong Port — No Traces
**Cause**: Phoenix 14.x serves OTLP on port 6006 (not 4318). Dockerfile had wrong port.  
**Fix**: Updated `PHOENIX_COLLECTOR_ENDPOINT` to `http://phoenix-observability:6006/v1/traces`.

### Bug 5: Input Parsing — `kind` vs `type`
**Cause**: Platform sends `kind: "text"` but agent only checked `type: "text"`.  
**Fix**: Added `kind` check alongside `type`.

### Bug 6: Method Handling — `message/send` Not Supported
**Cause**: Platform sends `message/send` but agent only handled `tasks/send`.  
**Fix**: Accept both methods.

---

## Deployment Pipeline (How It Works)

1. `scripts/deploy.py` builds zip from `examples/mcp-calculator-server-v2/`
2. Extracts to `nasiko/agents/mcpcalc/v1.0.0/` (runtime only, not committed)
3. Publishes deploy command to Redis stream `orchestration:commands`
4. Redis Listener (`nasiko-redis-listener`) picks it up and:
   - Copies agent code to `/tmp/agent-builds/mcpcalc-<ts>/`
   - Injects observability (AST-based, adds `bootstrap_tracing()` to `__main__.py`)
   - Builds Docker image `local-agent-mcpcalc:latest`
   - Runs container on `agents-net` + `app-network`
   - Registers in MongoDB registry
   - Creates permissions

---

## Phoenix Observability Guide

### View traces in browser:
1. Open `http://localhost:6006`
2. Click **"Tracing"** in left sidebar
3. Click the **mcp-calculator** project
4. Each row = one request. Click to see attributes:
   - `mcp.tool.query` — user's question
   - `mcp.tool.operation` — detected operation
   - `mcp.tool.result` — the answer

### Generate test traces:
```powershell
py -3 scripts/test.py
```

### Check traces via terminal:
```powershell
docker logs phoenix-observability --since "5m" 2>&1 | Select-String "v1/traces"
```

---

## Key Platform Code References (Read-Only)

| File | What It Does |
|------|-------------|
| `agent-gateway/router/src/core/agent_client.py` | Calls agent, requires `result.kind` = "message" |
| `agent-gateway/router/src/utils/message_utils.py` | Extracts text, only reads `kind: "text"` |
| `orchestrator/redis_stream_listener.py` | Builds & deploys containers, registers in registry |
| `app/utils/observability/injector.py` | AST-based tracing injection into `__main__.py` |

---

## Known Limitations

1. **Web UI chat** may not render response bubbles (Flutter rendering issue). Terminal works.
2. **Database status warnings** on manual deploys (`404`) are harmless — no upload record exists.
3. **Container may stop** if Docker Desktop restarts. Fix: `docker start agent-mcpcalc`.
4. **AST injection** rewrites `__main__.py` with `astor`. Our `CMD` runs `main.py` so this is safe.
