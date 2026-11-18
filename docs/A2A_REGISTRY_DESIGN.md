# A2A Registry Design for Nasiko Control Plane

## Protocol Constraints (from A2A spec)

The A2A protocol defines:
- **Agent Card** — the unit of registration (name, skills, capabilities, security schemes, interfaces)
- **Well-Known URL** — `/.well-known/agent-card.json` served via unauthenticated GET (ONLY standard URL)
- **Extended Agent Card** — richer card returned after authentication
- **No standard registry API** — registry CRUD, query, and discovery APIs are entirely implementation-defined

The spec is pull-based: clients fetch cards from well-known URLs or query registries. There is no spec-defined push/registration webhook or discovery query format.

## Key Decisions

1. **All inter-agent communication goes through control plane proxy** (no direct agent-to-agent)
2. **Agent skills are immutable per deployment** (new skills = new deploy)
3. **Discovery returns a directory** (agents don't need to know other agents by name). Filtering by capability/tags is design intent and **not implemented yet**: the registry returns every agent the caller may see, and the caller picks from the skills and descriptions
4. **Agents call other agents through the control plane proxy.** Discovery answers give each agent's proxy path, `/api/agents/{id}`, never a runtime address
5. **The registry IS an A2A agent** — agents discover other agents using the same A2A protocol they use for everything else

## Agent Contract (What We Enforce)

To run on the platform, an agent container must:
1. Serve `/.well-known/agent-card.json` with a valid AgentCard schema
2. Implement A2A JSON-RPC at the declared interface URL (at minimum: `SendMessage`)
3. Respond to health checks (HTTP 200 on the A2A endpoint)

That's it. The control plane cannot and does not enforce:
- Internal implementation, framework, or LLM choice
- Quality of responses
- Whether declared skills actually work (developer's responsibility)

## Registration Flow

When the control plane deploys an agent container:
1. Creates the container. **Seeded agents** (`SEED_AGENTS`) are given the discovery env var
   (`server/src/seed.rs`); other deployments are not, and an agent that wants discovery must be
   configured with the server's base URL itself:
   ```
   A2A_DISCOVERY_URL=http://cp:8080    ← base URL of the control plane; the registry is {base}/a2a/v1
   ```
2. Waits for health check to pass
3. Fetches `/.well-known/agent-card.json` from the agent's private IP
4. Validates the card schema
5. Stores card in registry database
6. Card is now discoverable by other agents

Skills are frozen at this point. To update skills → deploy new version.

## Discovery: Registry as an A2A Agent

The registry speaks A2A JSON-RPC. Agents discover other agents by sending it an A2A message — no proprietary REST API needed.

The only registry route is `POST /a2a/v1` (`server/src/lib.rs`). It accepts the methods
`message/send`, `message/stream` and `SendMessage` (any other method is JSON-RPC `-32601`), ignores
the message content, and always answers with the full directory the caller may see
(`server/src/registry_a2a.rs`).

### Registry Agent Card (not implemented)

The registry does **not** serve an agent card today; there is no
`GET {A2A_DISCOVERY_URL}/.well-known/agent-card.json` route. The card below, and the skills it
lists (capability/tag filtering, card lookup by name), are design intent:

```json
{
  "name": "Nasiko Agent Registry",
  "description": "Discovers and lists agents by capability, tags, or natural language query",
  "version": "1.0.0",
  "supportedInterfaces": [
    {
      "url": "http://cp:8080/a2a/v1",
      "protocolBinding": "JSONRPC",
      "protocolVersion": "1.0"
    }
  ],
  "capabilities": {
    "streaming": false,
    "pushNotifications": false
  },
  "defaultInputModes": ["application/json", "text/plain"],
  "defaultOutputModes": ["application/json"],
  "skills": [
    {
      "id": "discover-by-capability",
      "name": "Discover Agents by Capability",
      "description": "Find agents that match given tags, capabilities, or natural language description",
      "tags": ["discovery", "registry", "a2a", "search"],
      "examples": [
        "Find agents that can translate text",
        "Which agents support streaming?",
        "List all agents with tag: summarization"
      ],
      "inputModes": ["application/json", "text/plain"],
      "outputModes": ["application/json"]
    },
    {
      "id": "get-agent-card",
      "name": "Get Agent Card",
      "description": "Retrieve the full Agent Card for a specific agent by ID or name",
      "tags": ["discovery", "registry", "lookup"],
      "inputModes": ["application/json", "text/plain"],
      "outputModes": ["application/json"]
    },
    {
      "id": "list-agents",
      "name": "List All Agents",
      "description": "List all active agents with their skills and endpoints",
      "tags": ["discovery", "registry", "list"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    }
  ]
}
```

### Discovery Flow (Agent → Registry via A2A)

**Step 1: Agent knows the registry's address**

There is no registry card to fetch (see above); the agent posts straight to
`{A2A_DISCOVERY_URL}/a2a/v1`.

**Step 2: Agent queries registry using A2A SendMessage**

The message content is not read today: both requests below, and any other, return the same
directory. The structured `filter` is design intent, **not implemented**.

Structured query (filter not implemented):
```json
POST http://cp:8080/a2a/v1
A2A-Version: 1.0
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "method": "SendMessage",
  "params": {
    "message": {
      "role": "ROLE_USER",
      "parts": [
        {
          "data": {
            "action": "discover",
            "filter": {
              "tags": ["translation", "multilingual"],
              "capabilities": { "streaming": true }
            }
          }
        }
      ]
    }
  }
}
```

Natural language query (accepted; the text is not used for matching):
```json
POST http://cp:8080/a2a/v1
A2A-Version: 1.0
{
  "jsonrpc": "2.0",
  "id": "req-2",
  "method": "SendMessage",
  "params": {
    "message": {
      "role": "ROLE_USER",
      "parts": [
        { "text": "find agents that can translate between languages" }
      ]
    }
  }
}
```

**Step 3: Registry responds with the agent directory**

The result is a bare `artifacts` list (no task, status or `contextId`); the agents are at
`result.artifacts[0].parts[0].data.agents`. Each entry carries the id twice (`id`, `agent_id`) and
the proxy path twice (`url`, `endpoint`) so both in-tree consumers can read it; there is no
`capabilities` field.
```json
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "result": {
    "artifacts": [
      {
        "name": "agent-directory",
        "parts": [
          {
            "kind": "data",
            "data": {
              "agents": [
                {
                  "id": "translation-agent-uuid",
                  "agent_id": "translation-agent-uuid",
                  "name": "Translation Agent",
                  "description": "Translates between 40+ languages",
                  "url": "/api/agents/translation-agent-uuid",
                  "endpoint": "/api/agents/translation-agent-uuid",
                  "skills": [
                    {
                      "id": "translate-text",
                      "name": "Text Translation",
                      "description": "Translate text between languages",
                      "tags": ["translation", "nlp", "multilingual"],
                      "examples": ["Translate 'hello' to French"]
                    }
                  ]
                }
              ]
            }
          }
        ]
      }
    ]
  }
}
```

**Who sees what.** A caller with no credentials gets the public agents only. A caller that sends a
session token (`Authorization: Bearer`, or the session cookie) gets the agents that token may call
through the proxy, by the same access check the proxy applies; an invalid token is refused with
401. Every `endpoint` is the agent proxy path, relative to the control plane; an agent's internal
address is never returned. The route is rate-limited to 60 requests per minute in one global
window shared by every caller.

**Step 4: Agent calls discovered agent (standard A2A, through proxy)**
```json
POST http://cp:8080/api/agents/translation-agent-uuid
A2A-Version: 1.0
{
  "jsonrpc": "2.0",
  "id": "req-3",
  "method": "SendMessage",
  "params": {
    "message": {
      "role": "ROLE_USER",
      "parts": [
        { "text": "Translate 'hello world' to Spanish" }
      ]
    }
  }
}
```

### Authentication and Access Control

- **No token:** the registry lists public agents only.
- **A session token** (`Authorization: Bearer` or the session cookie): it lists the agents that caller may call through the agent proxy, decided by the same check the proxy uses. An invalid token is a 401, never a quiet fall back to the public view.
- Every entry's address is the agent proxy path, `/api/agents/{id}`, never a container address, so a call made from the list goes through the proxy's access check, flow guard and tracing (`server/src/registry_a2a.rs`).
- **Rate limit:** 60 requests per minute, one global fixed window for the route.

## End-to-End: Every Path is A2A

```
┌──────────────────────────────────────────────────────────────────────┐
│                        CONTROL PLANE                                  │
│                                                                      │
│  ┌──────────────────┐    ┌───────────┐    ┌─────────────────────┐   │
│  │ Registry         │    │   Proxy   │    │ Agent Card Store    │   │
│  │                  │    │           │    │ (PostgreSQL)        │   │
│  │ POST /a2a/v1     │    │ /api/     │    │                     │   │
│  │                  │    │  agents/  │    │                     │   │
│  │                  │    │  {id}     │    │                     │   │
│  └──────────────────┘    └───────────┘    └─────────────────────┘   │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘

Agent Developer's mental model:

  "There's one URL in my env var.
   I talk to it via A2A to discover other agents.
   It gives me endpoints. I talk to those endpoints via A2A.
   Everything is A2A. I don't learn any proprietary API."
```

## Well-Known URL Proxy (for external A2A clients)

For agents to be discoverable by external A2A clients (not running on the platform), a per-agent well-known proxy is planned but **not implemented yet**:

```
GET https://platform.example.com/agents/{agent-id}/.well-known/agent-card.json   ← not implemented
```

It would return the Agent Card with `supportedInterfaces[].url` pointing to the public proxy:
```json
{
  "supportedInterfaces": [
    {
      "url": "https://platform.example.com/api/agents/{agent-id}",
      "protocolBinding": "JSONRPC",
      "protocolVersion": "1.0"
    }
  ]
}
```

External clients can also discover agents by talking to the registry at:
```
POST https://platform.example.com/a2a/v1                        ← query the registry
```
(The registry serves no card of its own; see above.)

## Proxied Communication Flow

```
┌──────────┐         ┌───────────────────┐         ┌──────────┐
│ Agent A  │         │   Control Plane   │         │ Agent B  │
│          │         │                   │         │          │
│ 1. (no card fetch: the registry URL    │         │          │
│    comes from A2A_DISCOVERY_URL)       │         │          │
│          │         │                   │         │          │
│ 2. Query registry (A2A SendMessage)    │         │          │
│ ─────────────────► │                   │         │          │
│ POST /a2a/v1       │                   │         │          │
│ (content ignored)  │                   │         │          │
│          │         │                   │         │          │
│ ◄─────────────────── 3. Returns agents │         │          │
│ [{endpoint:        │    with proxy URLs│         │          │
│   "/api/agents/B"}]│                   │         │          │
│          │         │                   │         │          │
│ 4. Call Agent B (A2A SendMessage)      │         │          │
│ ─────────────────► │ 5. Authenticate   │         │          │
│ POST /api/agents/B │ 6. Check ACL      │         │          │
│          │         │ 7. Log interaction│         │          │
│          │         │ 8. Forward ──────────────►  │          │
│          │         │    POST /a2a/v1   │         │          │
│          │         │                   │         │          │
│          │         │ ◄──────────────────── 9. Response      │
│ ◄─────────────────── 10. Return to A   │         │          │
│          │         │                   │         │          │
└──────────┘         └───────────────────┘         └──────────┘

Every arrow is A2A protocol. Steps 1-4 from Agent A's perspective
are indistinguishable from talking to any other A2A agent.
```

### What the proxy does on every call (steps 5-8):

1. **Authenticates the caller** — a user's session, or an agent calling a peer. An agent sends its
   own deploy-time `MCP_GATEWAY_TOKEN` as `Authorization: Bearer`, which proves *which agent* is
   calling. *Which user* it works for is never taken from the request: it is resolved from the
   `traceparent` against `flows.user_id`, and the agent must be a recorded participant of that
   live flow (the same rule the MCP gateway and LLM router apply). No `traceparent`, or a flow the
   agent is not part of, is a 403. The call then runs as that user, so every user-level ACL
   applies too. Both `/api/agents/{id}` and `/api/agents/{id}/` reach the agent's A2A mount.
2. **Checks agent-to-agent ACL** — `check_agent_acl(caller_agent_id, target_agent_id)` against the `agent_acl` table,
   before the flow guard spends any budget and before a parked target is woken.
   Allowlist semantics: no rows for caller = unrestricted; any rows = only listed targets allowed.
   Enforced in the proxy (`server/src/agent_proxy.rs`) and inside `CpCallGuard.before_call()` (`server/src/acl.rs`).
   Distinct from user-to-agent access (`agent_grants` / `is_public`), which gates API-level access.
3. **Logs the interaction** — caller, target, timestamp, latency, status (audit trail)
4. **Rate limits** — an agent caller is limited per calling agent (keyed by its id) in the same fixed-window
   budget `AGENT_PEER_CALL_RATE_LIMIT_PER_MINUTE` (default 300), so a rogue agent cannot flood its peers (429 with `Retry-After`)
   Agents cannot drive a harness or a legacy coding agent through the proxy: such a call (including its
   `tasks/*` methods) is refused with JSON-RPC `-32600` before anything is written. An external agent
   calling a discovered URL must present a user JWT or an agent credential (`MCP_GATEWAY_TOKEN`, with a
   `traceparent` naming a flow it participates in); the in-tree assistant-agent sends its own token.
5. **Forwards request** — to the target resolved by kind (`server/src/agent_resolver.rs`): the
   agent's own workload at its live address. A **parked** agent (its workload scaled to zero while
   idle) is woken first by this very request (`server/src/agent_wake.rs`), which waits until it
   answers. A harness agent has no address at all; its turns run in a session sandbox. The registry therefore returns the proxy URL
   (`{base}/api/agents/{id}`) for **every** agent, never a container address, and calling a
   parked or harness agent at a direct URL is unsupported.
6. **Returns response** — the agent's response headers and body are passed back to the caller
   as they are; only the *request* headers are allowlisted before forwarding

## Registry Data Model

The live schema is in `migrations/`. Key tables:

```sql
-- Agent registry (one row per registered agent)
-- agents.id is UUID (original schema); name is the human/A2A identifier.
-- name is unique PER OWNER among active rows (partial unique index on
-- (owner_id, name) WHERE deleted_at IS NULL) — NOT globally unique.
-- search_vector is a GENERATED tsvector — never SELECT *.
CREATE TABLE agents (
    id                UUID    PRIMARY KEY DEFAULT gen_random_uuid(),
    name              TEXT    NOT NULL,
    description       TEXT,
    owner_id          UUID    NOT NULL REFERENCES users(id),
    url               TEXT,                            -- agent's A2A endpoint (private IP)
    capabilities      JSONB   NOT NULL DEFAULT '{}',
    skills            JSONB   NOT NULL DEFAULT '[]',  -- denormalised for fast card serialisation
    tags              TEXT[]  NOT NULL DEFAULT '{}',
    is_public         BOOLEAN NOT NULL DEFAULT FALSE, -- replaces Redis agent:{id}:public
    status            TEXT    NOT NULL DEFAULT 'registered', -- registered|running|parked|stopped|failed
    secrets_env       JSONB   NOT NULL DEFAULT '{}',  -- {key: aes_gcm_ciphertext} encrypted with agent-scoped HKDF key
    deleted_at        TIMESTAMPTZ,
    -- ... other columns omitted for brevity; see migrations/0001_schema.sql
    search_vector     tsvector GENERATED ALWAYS AS (...) STORED  -- exclude from SELECT lists
);

-- Normalised skills (mirrors agents.skills JSONB for indexed querying)
CREATE TABLE agent_skills (
    id        UUID PRIMARY KEY,
    agent_id  UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    skill_key VARCHAR(255) NOT NULL,
    name      VARCHAR(255) NOT NULL,
    tags      TEXT[] NOT NULL DEFAULT '{}',
    examples  JSONB  NOT NULL DEFAULT '[]',
    UNIQUE (agent_id, skill_key)
);

-- User grants for agent access
-- grant_type: 'user' | 'public'
-- grantee_id: UUID string for user grants, '*' for public
-- (The Nasiko enterprise edition extends grants to teams, departments,
--  and organization-wide access.)
CREATE TABLE agent_grants (
    id         UUID       PRIMARY KEY,
    agent_id   UUID       NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    grant_type grant_type NOT NULL,
    grantee_id TEXT       NOT NULL,
    granted_by UUID       REFERENCES users(id),
    UNIQUE (agent_id, grant_type, grantee_id)
);

-- Agent-to-agent invocation allowlist
-- No rows for caller → unrestricted. Any rows → only listed targets.
CREATE TABLE agent_acl (
    caller_agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    target_agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    granted_by      UUID REFERENCES users(id),
    PRIMARY KEY (caller_agent_id, target_agent_id)
);
```

## Health & Liveness

Discovery lists only agents whose status is `running` or `parked` (the proxy wakes a parked agent
on its next call) and that are not deleted or platform-internal
(`server/src/registry_a2a.rs`). Card content is not re-read for skill changes (skills are immutable
per deploy).

## Admin/UI Access (Non-A2A, Internal)

The web UI and CLI use standard REST for management operations (not A2A):

```
GET    /api/agents                     ← list agents (with status)
POST   /api/agents                     ← register an agent in the catalog
GET    /api/agents/{id}                ← agent details
PUT    /api/agents/{id}                ← update agent metadata
DELETE /api/agents/{id}                ← delete an agent
GET    /api/agents/by-skill            ← find agents by skill
POST   /api/agents/upload              ← upload source for a server-side build
       /api/agents/{id}/...            ← update, rollback, ownership and lifecycle operations
       /api/containers/*               ← container operations
```

The routes are defined in `server/src/agents/` and `server/src/catalog/`.

These are control plane internal APIs. They don't need to be A2A because they're for platform operators, not agents.

## Orchestrator Integration

The chat orchestrator is an internal component of the control plane (not a separate agent). It uses the registry data directly (DB query, no A2A round-trip):

1. User sends message to control plane
2. Orchestrator queries PostgreSQL for the running and parked agents the user may access
3. Orchestrator gives those agents, as A2A tools, plus the user message to an LLM (a ReAct loop)
4. The LLM calls one or more agents, or answers directly
5. Control plane forwards each call through the same checks as the proxy
6. Response streamed back to user

See `ROUTING_ENGINE.md` for this path and for the 3-stage routing engine MAF uses.

The orchestrator bypasses the A2A discovery protocol because it's inside the control plane — it has direct DB access. Only external agents and clients use the A2A discovery path.
