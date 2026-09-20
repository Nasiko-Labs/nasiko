# Agent Storage Design

## Principle

Containers are stateless. Persistence is a platform service, not a volume mount.
Agents get built-in persistence by virtue of speaking A2A correctly — the control plane
handles the rest.

## Three Levels of Persistence

### 1. Context Replay (automatic, zero agent effort)

The control plane proxies all A2A interactions and stores every message
keyed by `contextId` (a first-class A2A field). When a container restarts:

1. Next request arrives with the same `contextId`
2. Control plane looks up message history for that context
3. Replays previous turns as part of the A2A request params
4. Agent resumes — never knows it restarted

**Backed by:** PostgreSQL (structured) + object store (file parts)

**Agent developer does:** Nothing. Just implement A2A correctly.

### 2. Artifact Persistence (automatic via FileWithUri)

When an agent returns `FileWithUri` in an artifact:

1. Control plane intercepts the response
2. If URI points to an ephemeral source, stores the file in object storage (S3/MinIO)
3. Rewrites URI to a platform-managed, durable URL
4. Returns rewritten URI to the caller

Files survive container restarts, redeployments, and node failures.

**Backed by:** S3/MinIO

**Agent developer does:** Return `FileWithUri` in artifacts. Platform makes them permanent.

### 3. Storage Agent (opt-in, explicit read/write)

> **Status: design proposal, not yet implemented.** No Storage Agent ships with the
> platform today.

For agents that need active key-value persistence (preferences, computed state,
caches that shouldn't be recomputed), the platform provides a **Storage Agent**
discoverable via the registry.

```
Agent → Registry (A2A): "find agents with tag: storage"
Agent → Storage Agent (A2A): { action: "put", key: "...", value: {...} }
Agent → Storage Agent (A2A): { action: "get", key: "..." }
```

#### Storage Agent Skills

| Skill | Description |
|-------|-------------|
| `put` | Store a JSON value or file at a key |
| `get` | Retrieve a value by key |
| `delete` | Remove a key |
| `list` | List keys by prefix |

#### Key Namespacing

Keys are automatically scoped by the calling agent's ID:

```
Agent "weather-bot" stores key "prefs/units"
→ Stored as: /agents/weather-bot/prefs/units

Agent "weather-bot" cannot access /agents/other-agent/...
```

Enforced by the proxy (same ACL mechanism as inter-agent calls).

#### Sharing Data Between Agents

Agents share data by returning `FileWithUri` artifacts in A2A responses.
The proxy rewrites URIs to scoped, time-limited tokens. No direct cross-agent
storage access — sharing is always explicit through A2A message exchange.

## Vector Store / RAG

> **Status: design proposal, not yet implemented.** No Vector Store Agent ships with
> the platform today.

Same pattern — a **Vector Store Agent** on the platform:

```
Agent → Registry: "find agents with tag: vector-store"
Agent → Vector Agent: { action: "index", documents: [...] }
Agent → Vector Agent: { action: "search", query: "...", top_k: 5 }
```

Backed by pgvector, Qdrant, or similar. Agent doesn't know or care.

## Platform Services as A2A Agents

```
┌─────────────────────────────────────────────────────────────┐
│                    CONTROL PLANE                              │
│                                                             │
│  Platform Agents (internal, auto-deployed):                 │
│                                                             │
│  ┌────────────┐  ┌────────────┐  ┌────────────────────┐   │
│  │  Registry  │  │  Storage   │  │  Vector Store      │   │
│  │  Agent     │  │  Agent     │  │  Agent             │   │
│  │            │  │            │  │                    │   │
│  │ discovery  │  │ key-value  │  │ embeddings/search  │   │
│  │ by A2A     │  │ by A2A     │  │ by A2A             │   │
│  └────────────┘  └────────────┘  └────────────────────┘   │
│                                                             │
│  User Agents (deployed by users):                           │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐          │
│  │ Agent A    │  │ Agent B    │  │ Agent C    │          │
│  │            │  │            │  │            │          │
│  │ Discovers  │  │ Uses       │  │ Uses       │          │
│  │ & uses all │  │ storage +  │  │ vector     │          │
│  │ via A2A    │  │ vector     │  │ store      │          │
│  └────────────┘  └────────────┘  └────────────┘          │
│                                                             │
└─────────────────────────────────────────────────────────────┘

Every arrow is A2A. No proprietary SDKs. No volume mounts.
```

## What This Means for the Orchestrator

- **No volumes in ContainerSpec** — containers are pure compute
- **No data migration on reschedule** — move containers freely between nodes
- **Storage is a control plane concern** — S3/MinIO + Postgres, managed centrally
- **Agents are fungible** — any replica can serve any request (context replayed)

## Environment Variables Injected by Control Plane

```
PORT=8000                                   # port the agent must bind to
A2A_DISCOVERY_URL=http://10.0.0.1:8000/a2a/v1  # agent discovery registry endpoint
OPENAI_API_KEY=...                          # OPENAI_* LLM credentials/config
OTEL_EXPORTER_OTLP_ENDPOINT=...             # observability export target
```

Platform services (like a future Storage Agent) would be reached through the agent
proxy at `/api/agents/{id}` after discovery via `A2A_DISCOVERY_URL` — no dedicated
storage env var is injected.
