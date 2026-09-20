# OSS Contribution Copilot

An A2A agent that wraps the OSS Contribution Copilot — a LangGraph orchestrator driving Claude Agent SDK sessions that finds, claims, plans, implements and ships real open-source contributions, entirely under human approval gates.

## What it does

Given a repository, the copilot ingests it, triages its open issues, and — once a human approves — plans a fix, builds it in a sandbox, runs the tests, opens a pull request, and watches that pull request through to merge, answering the maintainer along the way. This agent is a thin adapter: it exposes that system's existing HTTP API as four A2A skills, and adds no capability of its own. It cannot write to GitHub, cannot skip a gate, and cannot widen the copilot's writable-repo allow-list, because it contains no code that does any of those things — every call here forwards to a route the copilot already serves.

## The four gates

Nothing here becomes public without a human:

| Gate | What it guards |
|---|---|
| **G1 — claim** | Posting a claim comment on an issue and starting work |
| **G2 — plan** | The plan the Builder will implement |
| **G3 — pr** | Pushing the branch and opening the pull request |
| **G4 — reply** | Every public reply to a maintainer on an open pull request |

`decide_approval` only forwards a decision a human already made — it never makes one, and a decision can only land on the gate it names (the copilot enforces this on its own side).

## The allow-list

The copilot writes only to repositories a human has explicitly granted, read fresh on every call from `config.yaml ∪ data/writable_grants.json`. One repository per grant, no wildcards, and an unreadable grants file authorises nothing. This agent has no path around it — it calls the same `/api/writable`-gated routes the copilot's own UI does.

## Skills (A2A)

| Skill | What it does |
|---|---|
| `contribute` | Start a contribution against a named repository |
| `contribution_status` | Current phase and any pending gate for a thread |
| `list_approvals` | Every open gate across threads |
| `decide_approval` | Record a human decision on a named gate |

Each skill returns a small projection — headline, gate, cost, and a link to the full detail in Mission Control — never the raw thread state, so every turn stays cheap for a caller like DronaHQ.

A REST façade for tool builders that prefer plain HTTP over JSONRPC is mounted on the same app, at both the bare path and under `/a2a/`:

```
POST /contribute   {"repo": "owner/name"}
GET  /status        ?thread_id=...
GET  /approvals
POST /approve        {"thread_id", "gate_id", "decision", "note"}
```

## Quick start

```bash
cp .env.example .env   # fill in ANTHROPIC_API_KEY, GITHUB_TOKEN — see the repo root's own .env
python src/__main__.py --host 0.0.0.0 --port 10010
```

The copilot itself must already be running (`make run-live` in the repository root, default `http://127.0.0.1:8000`); point this agent at it with `COPILOT_URL` if it runs elsewhere.

## Docker

```bash
docker compose up
```

## Environment variables

| Variable | Required | Description |
|----------|----------|-------------|
| `ANTHROPIC_API_KEY` | Yes | Passed through to the copilot process; never baked into this image |
| `GITHUB_TOKEN` | Yes | Same |
| `COPILOT_URL` | No | The copilot's own API, default `http://127.0.0.1:8000` |
| `MISSION_CONTROL_URL` | No | Used to build each projection's `detail_url`, default `http://localhost:5173` |
| `DRONAHQ_WEBHOOK_URL` / `DRONAHQ_API_KEY` | No | If set, a courtesy, fire-and-forget notification is posted here whenever a gate opens |
| `ADAPTER_API_KEY` | No | If set, the four REST façade routes require it as an `api-key` header — set this before exposing the agent through a public tunnel |

## A2A endpoint

`http://localhost:10010/`

## A note on how this was validated

This agent was built and validated standalone against the running copilot (`:8000` -> this agent on `:10010`) rather than through Nasiko's own control plane. Building the control plane from source filled the author's disk during the build window (the Rust build cache reached ~3.2 GB against ~4 GB free, and containerd remounted read-only twice); it is a hard capacity limit on that machine, not a fault in this agent. The `AgentCard.json` here conforms field-for-field to the contract read from `agents/claude-sdk/` and `agents/langgraph/`, and is deployable as-is by anyone whose disk can host the control plane's build.
