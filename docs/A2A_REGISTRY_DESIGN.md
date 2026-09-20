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
3. **Discovery is by capability/tags** (agents don't need to know other agents by name)
4. **Agents never learn private IPs of other agents** (only proxy URLs)
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
1. Creates container with the injected discovery env var:
   ```
   A2A_DISCOVERY_URL=http://cp:8080    ← base URL of the registry agent
   ```
2. Waits for health check to pass
3. Fetches `/.well-known/agent-card.json` from the agent's private IP
4. Validates the card schema
5. Stores card in registry database
6. Card is now discoverable by other agents

Skills are frozen at this point. To update skills → deploy new version.

## Discovery: Registry as an A2A Agent

The registry is itself an A2A-compliant agent. Agents discover other agents by talking A2A to it — no proprietary REST API needed.

### Registry Agent Card

```
GET {A2A_DISCOVERY_URL}/.well-known/agent-card.json
```

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

**Step 1: Agent fetches registry's card (standard A2A discovery)**
```
GET http://cp:8080/.well-known/agent-card.json
→ Gets the registry agent card, learns its A2A endpoint
```

**Step 2: Agent queries registry using A2A SendMessage**

Structured query (preferred for programmatic use):
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

Natural language query (also works):
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

**Step 3: Registry responds with matching agents (standard A2A response)**
```json
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "result": {
    "task": {
    "id": "task-uuid",
    "contextId": "ctx-uuid",
    "status": { "state": "TASK_STATE_COMPLETED" },
    "artifacts": [
      {
        "parts": [
          {
            "kind": "data",
            "data": {
              "agents": [
                {
                  "agent_id": "translation-agent-uuid",
                  "name": "Translation Agent",
                  "description": "Translates between 40+ languages",
                  "skills": [
                    {
                      "id": "translate-text",
                      "name": "Text Translation",
                      "tags": ["translation", "nlp", "multilingual"]
                    }
                  ],
                  "capabilities": { "streaming": true },
                  "endpoint": "http://cp:8080/api/agents/translation-agent-uuid"
                },
                {
                  "agent_id": "deepl-agent-uuid",
                  "name": "DeepL Agent",
                  "description": "High-quality translation via DeepL",
                  "skills": [
                    {
                      "id": "deepl-translate",
                      "name": "DeepL Translation",
                      "tags": ["translation", "multilingual"]
                    }
                  ],
                  "capabilities": { "streaming": false },
                  "endpoint": "http://cp:8080/api/agents/deepl-agent-uuid"
                }
              ]
            }
          }
        ]
      }
    ]}
  }
}
```

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

## End-to-End: Every Path is A2A

```
┌──────────────────────────────────────────────────────────────────────┐
│                        CONTROL PLANE                                  │
│                                                                      │
│  ┌──────────────────┐    ┌───────────┐    ┌─────────────────────┐   │
│  │ Registry Agent   │    │   Proxy   │    │ Agent Card Store    │   │
│  │                  │    │           │    │ (PostgreSQL)        │   │
│  │ /.well-known/    │    │ /api/     │    │                     │   │
│  │   agent-card.json│    │  agents/  │    │                     │   │
│  │ /a2a/v1          │    │  {id}     │    │                     │   │
│  └──────────────────┘    └───────────┘    └─────────────────────┘   │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘

Agent Developer's mental model:

  "There's one URL in my env var. I fetch its agent card.
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

External clients can also discover agents by talking to the registry agent at:
```
GET https://platform.example.com/.well-known/agent-card.json   ← registry's own card
POST https://platform.example.com/a2a/v1                        ← query the registry
```

## Proxied Communication Flow

```
┌──────────┐         ┌───────────────────┐         ┌──────────┐
│ Agent A  │         │   Control Plane   │         │ Agent B  │
│          │         │                   │         │          │
│ 1. Fetch registry card (A2A standard)  │         │          │
│ ─────────────────► │                   │         │          │
│ GET /.well-known/  │                   │         │          │
│   agent-card.json  │                   │         │          │
│          │         │                   │         │          │
│ 2. Query registry (A2A SendMessage)    │         │          │
│ ─────────────────► │                   │         │          │
│ POST /a2a/v1       │                   │         │          │
│ "find translation" │                   │         │          │
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

1. **Authenticates the caller** — the proxy identifies the calling agent before forwarding
2. **Checks agent-to-agent ACL** — `check_agent_acl(caller_agent_id, target_agent_id)` against the `agent_acl` table.
   Allowlist semantics: no rows for caller = unrestricted; any rows = only listed targets allowed.
   Enforced inside `CpCallGuard.before_call()` in `server/src/acl.rs`.
   Distinct from user-to-agent access (`agent_grants` / `is_public`), which gates API-level access.
3. **Logs the interaction** — caller, target, timestamp, latency, status (audit trail)
4. **Rate limits** — prevent a rogue agent from flooding another
5. **Forwards request** — to target agent's actual private IP:port
6. **Returns response** — strips internal headers, returns to caller

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
    status            TEXT    NOT NULL DEFAULT 'registered', -- registered|running|stopped|failed
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

Control plane polls every active agent every 30s:
- `GET {private_url}/.well-known/agent-card.json`
- If 3 consecutive failures → mark `unhealthy`, exclude from discovery results
- If VM is terminated → mark `removed`
- Card content is NOT re-read for skill changes (immutable per deploy)

## Admin/UI Access (Non-A2A, Internal)

The web UI and CLI use standard REST for management operations (not A2A):

```
GET    /api/agents                     ← list all agents (with status)
GET    /api/agents/{id}                ← full agent details
POST   /api/agents                     ← register/deploy new agent
PUT    /api/agents/{id}                ← update agent
DELETE /api/agents/{id}                ← stop and remove agent
POST   /api/agents/upload              ← upload source for a server-side build
       /api/containers/*               ← container ops (stop, start, restart, scale, logs)
```

These are control plane internal APIs. They don't need to be A2A because they're for platform operators, not agents.

## LLM Router Integration

The LLM router is an internal component of the control plane (not a separate agent). It uses the registry data directly (DB query, no A2A round-trip):

1. User sends message to control plane
2. Router queries PostgreSQL for all active agent cards
3. Router sends agent cards + user message to LLM
4. LLM picks the best agent based on skills/description
5. Control plane proxies the request to chosen agent
6. Response streamed back to user

The router bypasses the A2A discovery protocol because it's inside the control plane — it has direct DB access. Only external agents and clients use the A2A discovery path.
