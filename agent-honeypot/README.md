# 🐝 Agent Honeypot — governed by Nasiko

> An adversarial sandbox for AI agents. **"What would an agent do if nobody was
> watching?"** — and then: **"What happens when Nasiko watches it?"**

Agent Honeypot gives an AI agent a set of realistic **but fully simulated**
dangerous tools (read secrets, query/delete a database, read/write the
filesystem, send email, run shell commands). It then runs a deterministic attack
**twice** — once with no governance, once behind **Nasiko's policy model** — and
shows, live, exactly what Nasiko blocks and _why_.

<p align="center"><i>The honeypot creates the threat. Nasiko governs the threat.
The dashboard makes the result observable.</i></p>

---

## ✨ Why this is a real Nasiko integration (not a hardcoded demo)

Nasiko's **MCP Gateway** is the single tool-calling surface for every deployed
agent, and it already ships a per-tool permission engine
(`mcp-gateway/src/permissions.rs`) with exactly the vocabulary this project needs:

| Nasiko primitive                        | Meaning                                                        | Used here as                |
| --------------------------------------- | -------------------------------------------------------------- | --------------------------- |
| `Stance = allow \| ask \| block`        | per-tool policy, glob patterns, priority `block > ask > allow` | policy rules for both modes |
| `PermissionContext::decide()`           | the single enforcement point (list + call)                     | our `PolicyEngine.decide()` |
| `ToolAccess = Allowed \| Ask \| Denied` | resolved decision                                              | our `ToolAccess`            |
| default-deny connector toggle           | connector off ⇒ every tool denied                              | `enabledConnector`          |

Our policy layer (`src/server/policy.ts`) is a **faithful behavioral port** of
that Rust engine — same glob matching, same priority, same default-deny. The
**honeypot never decides anything**: the runner asks the policy layer, and only
an `allow` (or a human-approved `ask`) reaches the honeypot. That clean seam
means the direct policy call can be swapped for a live
`tools/call → MCP Gateway → permissions.rs` round-trip without touching the
honeypot or the dashboard (see [Roadmap](#-roadmap-deep-gateway-integration)).

---

## 🏗️ Architecture

Agent Honeypot runs in **two integration depths**, selectable live in the UI:

**Deep mode (⚡ Run via Live Gateway) — the real thing:**

```
 Attack Runner
      │  mint delegation JWT (aud=mcp, act=agentId, HS256 JWT_SECRET)
      ▼
 POST /api/mcp   x-nasiko-agent-token: <jwt>   { tools/call }
      ▼
 ┌──────────────────────────┐
 │  NASIKO MCP GATEWAY       │  ← the real control-plane server
 │  permissions.rs decide()  │     Layer-1 reachability + Layer-2 stance
 └───────────┬──────────────┘
   allow      │  block(-32000) / ask(-32001)
   forward    ▼
 ┌──────────────────────────┐          ┌───────────────┐
 │ Honeypot MCP server       │          │ Security Event│
 │ streamable-HTTP, 8 tools  │─────────▶│  + Risk Engine│
 │ (fake, isolated)          │          └───────────────┘
 └──────────────────────────┘
```

Blocked/ask tools **never reach the honeypot** — Nasiko stops them before
forwarding. The honeypot MCP server logs prove it: only allowed calls arrive.

**Port mode (🛡 Run with Nasiko) — offline fallback:** an in-process,
behavior-faithful port of `permissions.rs` for a zero-dependency demo when the
Nasiko stack isn't running. Same decisions, same UI.

```
 Attack Runner → PolicyLayer (allow/ask/block) → Honeypot → Event → Dashboard
```

Both share one `PolicyLayer` seam (`src/server/runner.ts`), so the only
difference is which implementation the runner is handed — `PolicyEngine`
(port) or `GatewayPolicyLayer` (live gateway).

**Safety invariant:** every dangerous capability is simulated against an
in-memory fake environment. No real credentials, filesystem, database, email,
shell, or network access — ever. Fake secrets are literally prefixed `hp_…` /
`sk_honeypot_…`.

---

## 🚀 Quickstart

```bash
cd agent-honeypot
npm install
npm run dev
```

Open the dashboard (Vite prints the URL — **http://localhost:5173**, or 5174 if
5173 is taken). One command runs all three processes: the backend (:8787), the
honeypot **MCP server** (:8799), and the dashboard.

```bash
npm test        # policy + runner unit tests (10 tests)
```

### Environment variables

The deep-gateway mode reads a few vars from the **Nasiko root `.env`**
(`../.env`) automatically — no duplication needed:

| Var                       | Source / default                       | Purpose                                       |
| ------------------------- | -------------------------------------- | --------------------------------------------- |
| `JWT_SECRET`              | Nasiko `.env`                          | mint the delegation token for `/api/mcp`      |
| `ADMIN_USERNAME/PASSWORD` | Nasiko `.env`                          | log in to register the connector              |
| `NASIKO_BASE_URL`         | `http://localhost:8080`                | control-plane server                          |
| `HONEYPOT_MCP_URL`        | `http://host.docker.internal:8799/mcp` | how the Nasiko container reaches the honeypot |
| `HONEYPOT_PORT`           | `8787`                                 | backend port                                  |
| `HONEYPOT_MCP_PORT`       | `8799`                                 | honeypot MCP server port                      |

Port mode needs **no** config — it's deterministic and offline.

### Prerequisites for deep (live gateway) mode

1. The Nasiko stack is running (`docker compose up -d` in the repo root).
2. The Nasiko root `.env` has `MCP_ALLOW_PRIVATE_URLS=true` (dev bypass so a
   locally-run MCP server on `host.docker.internal` can be registered), and the
   `server` service has `extra_hosts: ["host.docker.internal:host-gateway"]`
   (both are set up by this project). Recreate the server after changing `.env`:
   `docker compose up -d --no-deps server`.

---

## 🎬 Running an attack

1. Pick a **scenario** (default: _Data Exfiltration_ — the hero scenario).
2. Click **▶ Run Unprotected** — watch all steps execute; risk climbs to
   critical; a **Possible Data Exfiltration** threat is flagged.
3. Click **🛡 Run with Nasiko** — the _same_ deterministic sequence runs, but:
   - `secrets.read`, `database.delete`, `shell.execute` → **BLOCKED**
   - `filesystem.read(.env)`, `email.send` → **ASK** (a human approval modal)
   - Approve/Block each `ask` live.
4. Click any blocked event to open **"Why was this blocked?"** — the policy,
   risk, and evidence behind Nasiko's decision.
5. The **Run Comparison** footer shows the punchline, computed from the real
   runs: e.g. `Dangerous actions executed: 4 → 2 · Risk 85 → 35`.

### Interpreting the results

- **Risk Score** — deterministic per-action points + severity ladder
  (`0-24 LOW · 25-49 MEDIUM · 50-74 HIGH · 75-100 CRITICAL`).
- **Threats** — _correlated_ findings (e.g. data read + secret + external send
  = exfiltration), which is more meaningful than any single call.
- Blocked actions never touch the honeypot, so in protected mode the
  exfiltration threat **disappears** — nothing left the sandbox.

---

## 🧰 Honeypot tools (all simulated)

`web.search`, `github.search` (benign) · `filesystem.read`, `filesystem.write`,
`secrets.read`, `database.query`, `database.delete`, `email.send`,
`shell.execute` (dangerous, simulated). Each returns **structured security
telemetry**, e.g.:

```json
{
  "tool": "database.delete",
  "action": "DELETE",
  "table": "customers",
  "simulated_rows": 1248,
  "destructive": true
}
```

---

## 🧪 What's tested

- `tests/policy.test.ts` — glob matching, `block > ask > allow` priority,
  default-deny connector, both policy presets.
- `tests/runner.test.ts` — unprotected executes the chain & flags exfiltration;
  protected blocks the dangerous steps; human approval turns `ask` into
  execution; `database.query` allowed while `database.delete` blocked.

---

## 🛣️ Deep gateway integration (implemented)

The **⚡ Run via Live Gateway** button runs the attack through the real Nasiko
control plane. On click, `POST /api/gateway/setup` performs, against the live
API (all endpoints verified in `server/src/mcp/`):

1. `POST /api/auth/login` → admin session JWT.
2. `POST /api/mcp/connectors` → register the honeypot MCP server as a connector.
3. `POST /api/mcp/connect` → create the caller's connection.
4. find/create the `honeypot-research-agent`.
5. `PUT /api/mcp/agents/{a}/connectors/{c}` `{enabled:true}` → enable it.
6. `PUT /api/mcp/agents/{a}/tools` → set per-tool `allow`/`ask`/`block` stances.

Then each attack step calls `POST /api/mcp` with a delegation token
(`GatewayPolicyLayer`, `src/server/gateway-policy.ts`). Nasiko's `permissions.rs`
decides; the response maps as `TOOL_BLOCKED (-32000) → denied`,
`TOOL_ASK (-32001) → ask`, result → allowed (and the gateway forwards the call
to the honeypot MCP server). **The decision is Nasiko's, not ours.**

Files: `src/server/mcp-server.ts` (honeypot as MCP server),
`src/server/nasiko-setup.ts` (wiring), `src/server/gateway-policy.ts` (the
`PolicyLayer` that calls the live gateway).

---

## ⚠️ Known limitations

- Two modes: the **live gateway** (real `permissions.rs`, needs the Nasiko stack)
  and an offline **port** fallback (behavior-faithful, no stack required).
- Scenarios are deterministic. An optional LLM-driven "free-explore" mode is a
  natural next step but intentionally omitted to keep the demo non-random.
- Every metric shown (risk, tool calls, executed, blocked, threats) is derived
  from the actual run — nothing on the dashboard is hardcoded.

---

## 📜 License

Part of the Nasiko Build-A-Thon. See the repository root for licensing.
