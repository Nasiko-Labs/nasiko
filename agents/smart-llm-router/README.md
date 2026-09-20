# Smart LLM Router

**Core intent:** cut token spend by classifying each prompt and asking Nasiko for the
cheapest viable tier (`cheap` → Groq/small, `balanced`, `premium` only when needed).
Silly queries should not burn GPT‑4o.

This agent is the **brain** in front of Nasiko `POST /v1/route`. The Nasiko console
talks to it over **A2A on `/`** (AgentCard + JSON-RPC) — same as any other deployed
agent. Console replies are **chat-only** — cost savings vs always-GPT‑4o live in the
session store and the analytics dashboard, not in the message text.

An optional **analytics dashboard** lives at `/ui/` only. It is **not** the Nasiko
chat UI. Metrics stream to `/ui/` over SSE (or remote ingest).

## Architecture (who does what)

```
User (Nasiko console / nasiko chat)
        │  A2A JSON-RPC  (POST /)
        ▼
 Smart LLM Router agent  (deployed or :8000/)  ← classify + pick tier + call Nasiko
        │  publish_async (non-blocking)
        ├─ local process ──►  /ui SSE /api/events
        └─ deployed box  ──►  POST ANALYTICS_INGEST_URL → host :8000/api/ingest → /ui
        │  POST /v1/route  (x-nasiko-tier)
        ▼
 Nasiko LLM router       (:8080)     ← cascade to groq/mistral/openai/…
```

Console replies are chat-only. The analytics host at `http://127.0.0.1:8000/ui/`
receives turns over SSE (same process) or via `ANALYTICS_INGEST_URL` when the agent
runs inside Nasiko Docker (`host.docker.internal:8000/api/ingest` by default).
**Do not** set a classifier key to the agent’s OpenAI gateway JWT — that caused ~20s
timeouts; leave `CLASSIFIER_*` unset for the fast heuristic unless you have a real Groq key.
**DronaHQ** is the *integration partner* for the demo, not something we embed inside
Nasiko. Their agents builder implements the same node graph (classifier → tier JS →
REST → savings). On integration day they point `ROUTER_BASE_URL` at Nasiko’s
`/v1/route` (or our mock). We own the router contract; they own the DronaHQ-side
agent. This Python agent mirrors that brain on Nasiko so you can chat via the
console without leaving the Nasiko control plane.

DronaHQ is a low-code builder; Nasiko remains the control plane / A2A proxy.
We are not “running DronaHQ inside Nasiko” — we share a frozen HTTP contract.

## Quick start (against local Nasiko)

Nasiko should be up on `:8080` (live keys with `NASIKO_ROUTE_TOKEN` / agent JWT,
or `NASIKO_ROUTE_STUB=1`):

```bash
cd agents/smart-llm-router
python -m venv .venv && source .venv/bin/activate
pip install -e .

export ROUTER_BASE_URL=http://localhost:8080
# Match server NASIKO_ROUTE_TOKEN, or use NASIKO_ROUTE_STUB=1 for local demos
export NASIKO_ROUTE_TOKEN=change-me
python -m smart_llm_router.eval_demo
```

A2A server for Nasiko console:

```bash
python -m smart_llm_router --host 127.0.0.1 --port 8000
# AgentCard:  http://127.0.0.1:8000/.well-known/agent-card.json
# Optional demo UI only: http://127.0.0.1:8000/ui/
nasiko chat http://127.0.0.1:8000 "hi"
```

## Pipeline

1. **Classify** — small/cheap model (or heuristic) → `{task_type, complexity}`
2. **Tier** — ≤2 → `cheap`; hard reason/code → `premium`; else `balanced`
3. **Router** — `POST {{ROUTER_BASE_URL}}/v1/route` with `x-nasiko-tier`
4. **Savings** — GPT‑4o baseline − actual `cost_usd`; session counters

## Environment

| Variable | Default | Description |
| --- | --- | --- |
| `ROUTER_BASE_URL` | `http://localhost:8080` | Nasiko or DronaHQ mock (`:8090`) |
| `ANALYTICS_INGEST_URL` | — (Dockerfile: host `:8000`) | Deployed agent → host dashboard ingest |
| `ANALYTICS_TOKEN` | — | Optional shared secret for `/api/*` analytics routes |
| `CLASSIFIER_API_KEY` / `GROQ_API_KEY` | — | Optional; else heuristic (recommended) |
| `CLASSIFIER_BASE_URL` | Groq when keyed | Classifier chat-completions base |
| `CLASSIFIER_MODEL` | `llama-3.1-8b-instant` | Cheap classifier model |
| `ROUTER_TIMEOUT_SECS` | `30` | Downstream timeout |

## Evaluation

`python -m smart_llm_router.eval_demo` checks trivial→cheap, proof→premium, fallback, errors,
session savings, and env-only base URL.
