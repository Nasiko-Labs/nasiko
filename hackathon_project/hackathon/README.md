# NasikoGateway — LLM Router Gateway Integration

> **Hackathon Track 2** — Platform-managed LLM gateway for the [Nasiko](https://github.com/Nasiko-Labs/nasiko) open-source agent orchestration platform.

---

## What It Does

NasikoGateway introduces a **centralized LLM routing layer** powered by [LiteLLM Proxy](https://docs.litellm.ai/docs/proxy/quick_start). Instead of every agent managing its own provider API keys, agents talk to a single gateway endpoint using a **virtual key**. The gateway holds the real credentials and routes requests to whichever provider is configured — Gemini, GPT-4o, Claude, or Groq.

```
OLD (fragile, insecure)
agent code  →  hardcoded OPENAI_API_KEY  →  OpenAI only

NEW (portable, secure)
agent code  →  GATEWAY_VIRTUAL_KEY  →  LiteLLM Gateway  →  any provider
```

Switching from Claude to Gemini to GPT-4o = **one line change in `gateway-config.yaml`, zero agent code changes.**

---

## Quick Start

### Prerequisites
- Docker + Docker Compose
- `make`

### Start Everything

```bash
# Clone and start the full Nasiko stack with gateway
git clone https://github.com/Nasiko-Labs/nasiko
cd nasiko
make start-nasiko        # starts Nasiko + LiteLLM gateway + Postgres + Redis
```

### Or start just the gateway

```bash
make start-gateway       # only the LiteLLM proxy stack
```

### Generate a virtual key

```bash
make generate-key team=demo
# → sk-litellm-v1-xxxxxxxxxxxxxxxx
```

### Run the gateway agent

```bash
export GATEWAY_VIRTUAL_KEY=sk-litellm-v1-your-key-here
python agents/sample-agent-gateway.py
```

### Switch providers (zero code changes)

Edit `config/gateway-config.yaml`:
```yaml
router_settings:
  default_model: gpt-4o   # was: gemini-flash — this is the ONLY change
```

Then restart the gateway:
```bash
make restart-gateway
```

**The agent code is unchanged. It still calls `http://localhost:4000`.** The gateway routes to the new provider.

---

## Project Structure

```
hackathon_project/
├── config/
│   └── gateway-config.yaml        # LiteLLM multi-provider config (all providers here)
├── docker/
│   └── docker-compose.gateway.yml # LiteLLM + Postgres + Redis
├── agents/
│   ├── base_gateway_client.py     # Reusable gateway client base class
│   ├── sample-agent-gateway.py    #New pattern — virtual key only
│   └── sample-agent-legacy.py    # Legacy pattern — backward compatible
├── nasiko-agent/
│   └── src/main.py                # Nasiko-compliant FastAPI agent
├── frontend/
│   └── index.html                 # Live dashboard — provider switching, trace log
└── Makefile                       # make start-gateway / generate-key / restart-gateway
```

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Agent Code                               │
│  GATEWAY_URL = http://localhost:4000                            │
│  Authorization: Bearer sk-litellm-v1-xxx   (virtual key only)  │
└────────────────────────────┬────────────────────────────────────┘
                             │ OpenAI-compatible /chat/completions
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                    LiteLLM Proxy (port 4000)                    │
│  • Resolves virtual key → real provider key (in container env)  │
│  • Routes to configured default_model                           │
│  • Enforces budget / RPM / TPM limits per virtual key           │
│  • Emits OpenTelemetry spans → Arize Phoenix                    │
│  • Automatic fallback: gemini-flash → gpt-3.5 → claude-haiku   │
└──────┬───────────────────────┬────────────────────┬────────────┘
       │                       │                    │
       ▼                       ▼                    ▼
  Google Gemini            OpenAI                Anthropic / Groq
  (free tier)            (GPT-4o)            (Claude / Llama3)
```

---

## Key Design Decisions

### Why LiteLLM over Portkey?

| Criteria | LiteLLM | Portkey |
|---|---|---|
| Self-hosted | ✅ Full control | ⚠️ Cloud-first |
| Open source | ✅ Apache 2.0 | ❌ Proprietary |
| Provider support | 100+ | ~20 |
| Virtual keys | ✅ Built-in | ✅ Built-in |
| OpenTelemetry | ✅ Native | ✅ Native |
| Nasiko alignment | ✅ Composable | ⚠️ External dependency |

LiteLLM runs inside the same Docker network as Nasiko agents — no outbound SaaS dependency.

### Virtual Key Security

Real provider credentials (`ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, etc.) are:
- Stored only in the gateway container's environment (`.env` file, never committed)
- Never exposed to agents or agent source code
- Per-key budgets and rate limits enforced at gateway level

### Backward Compatibility

Legacy agents using `OPENAI_API_KEY` directly continue working without modification. The gateway adds a new pattern; it does not replace or break the existing one.

---

## Provider Configuration

Providers are defined in `config/gateway-config.yaml`:

```yaml
model_list:
  - model_name: gemini-flash       # alias agents use
    litellm_params:
      model: gemini/gemini-1.5-flash
      api_key: "os.environ/GEMINI_API_KEY"   # real key — only in gateway env
      rpm: 1000
      tpm: 1000000

  - model_name: gpt-4o
    litellm_params:
      model: openai/gpt-4o
      api_key: "os.environ/OPENAI_API_KEY"
```

To add a new provider: add a stanza, set the env var in `.env`, restart. **No agent changes required.**

---

## Multi-Provider Cost Guide

| Provider | Model | Approx cost | Best for |
|---|---|---|---|
| Google Gemini | gemini-1.5-flash | Free (rate limited) | Development, high volume |
| Groq | llama3-8b | Free (fast inference) | Low latency tasks |
| OpenAI | gpt-3.5-turbo | ~$0.001/1K tok | Cost-balanced production |
| OpenAI | gpt-4o | ~$0.005/1K tok | Complex reasoning |
| Anthropic | claude-3-haiku | ~$0.00025/1K tok | Fast, cost-efficient |
| Anthropic | claude-3-5-sonnet | ~$0.003/1K tok | Long context, coding |

Switching from GPT-4o to Gemini Flash for a 10M token/month workload: **~$50k/month → $0**.

---

## Observability

Requests are traced via **OpenTelemetry** → [Arize Phoenix](https://phoenix.arize.com/):

```yaml
litellm_settings:
  success_callback: ["arize_phoenix"]
  failure_callback: ["arize_phoenix"]
```

Each request produces correlated spans at both the agent call layer and the gateway routing layer, giving end-to-end latency visibility per provider.

---

## Acceptance Criteria

| Criterion | Status |
|---|---|
| Gateway auto-deploys via `make start-nasiko` | ✅ |
| Sample agent completes LLM call without provider key in source | ✅ |
| Changing provider = gateway-config only, no agent code change | ✅ |
| Existing agents keep working without modification | ✅ |
| Docs warn against hardcoding model keys | ✅ |
| LiteLLM choice documented with trade-off rationale | ✅ |

---

## Makefile Reference

```bash
make start-gateway       # Start LiteLLM + Postgres + Redis
make stop-gateway        # Stop gateway containers
make restart-gateway     # Restart (picks up config changes)
make generate-key team=X # Create virtual key for team X
make start-nasiko        # Start full Nasiko stack + gateway
make logs-gateway        # Tail gateway logs
make test-agents         # Run integration tests for both agent patterns
```

---

## Environment Variables

Create a `.env` file (never commit this):

```env
LITELLM_MASTER_KEY=sk-master-your-secret
OPENAI_API_KEY=sk-...
ANTHROPIC_API_KEY=sk-ant-...
GEMINI_API_KEY=AI...
GROQ_API_KEY=gsk_...
```

Agents only need:
```env
GATEWAY_URL=http://localhost:4000
GATEWAY_VIRTUAL_KEY=sk-litellm-v1-generated-key
```

---

## Security Notes

> ⚠️ **Never hardcode provider API keys in agent code or commit them to source control.**

The gateway pattern enforces this structurally: agents are issued virtual keys that carry no provider credentials. Real keys live only in the gateway container environment, isolated from agent source trees. Virtual keys can be rotated, budget-capped, and revoked without touching agent code.

---

## Tech Stack

- **[LiteLLM Proxy](https://docs.litellm.ai/)** — multi-provider LLM gateway
- **Docker + Docker Compose** — containerized gateway stack
- **FastAPI** — Nasiko-compliant agent server (`nasiko-agent/src/main.py`)
- **PostgreSQL + Redis** — virtual key storage and caching
- **OpenTelemetry + Arize Phoenix** — distributed tracing
- **Python 3.11** — agent runtime

---

## Team Bible Black

Built for **Nasiko Labs Hackathon — Track 2: LLM Router Gateway Integration**
