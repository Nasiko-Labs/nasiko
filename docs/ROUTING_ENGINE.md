# Routing Engine

How the orchestrator picks the right agent for a user query.

## Overview

When a user sends a message to `POST /api/orchestrator/a2a` without specifying an `agent_id` (or
with `agent_id = "orchestrator"`), the server runs a **3-stage semantic routing pipeline** to select
the best agent. The pipeline progressively narrows a large agent catalog down to a single agent
using embeddings, conversation context, and an LLM final decision.

```
  User query
      |
      v
  Fetch accessible agents from DB
      |
      v
  Stage 1: Vector Shortlist        (embedding cosine similarity)
      |
      v
  Stage 2: Conversation Reranking  (history-aware re-scoring)
      |
      v
  Stage 3: LLM Final Selection     (structured output, deterministic)
      |
      v
  Selected agent --> proxy request --> stream response
```

Every stage is **fail-safe**: if embeddings are unavailable, if history is empty, or if the LLM
call errors, the pipeline degrades gracefully instead of failing the request.

## Request Entry Point

**File:** `oss/server/src/router/a2a_dispatch.rs`

The dispatch handler inspects `metadata.agent_id` in the incoming JSON-RPC request:

| `agent_id` value | Behavior |
|---|---|
| absent or `"orchestrator"` | **Orchestrator path** -- runs the 3-stage pipeline |
| any other value (name or UUID) | **Direct path** -- resolves the agent, checks ACL, proxies directly |

For the orchestrator path the handler:

1. Fetches conversation history from the DB (last N messages, configurable via
   `max_router_history_messages`, default 20).
2. Enriches the current query with that history.
3. Collects all running agents accessible to the caller.
4. Passes candidates through the routing engine.
5. Proxies the request to the selected agent and streams SSE events back.

## Access Control (Pre-filter)

Before any routing stage, agents are filtered by access control. An agent is accessible if **any**
of these hold:

- The caller owns the agent (`agents.owner_id`).
- The agent is public (`agents.is_public`).
- An `agent_grants` row exists for the caller (user, org_unit, or organization grant).
- The caller is a superuser (skips all checks).
- **(EE)** Team/department grants via the org-unit hierarchy.

Soft-deleted agents are always excluded. The `agent_acl` allowlist table additionally restricts
which agents can call which other agents during inter-agent routing.

**File:** `oss/server/src/acl.rs` (the enterprise edition layers its grant matrix on top of this
through the `AuthService` trait).

## Stage 1: Vector Semantic Shortlist

**File:** `oss/orchestrator/src/vector_store.rs`

**Purpose:** Quickly narrow a large catalog (tens or hundreds of agents) to a small candidate set
using embedding similarity.

**When it runs:** Only when the number of accessible agents >= `ROUTER_SHORTLIST_THRESHOLD`
(default 15). For smaller catalogs the stage is skipped and all agents advance.

### How it works

1. **Build agent text** for embedding:
   ```
   "{name} {description} {tags joined by space}"
   ```

2. **Embed agents** via the configured embedding provider (default: OpenAI
   `text-embedding-3-small`). Results are cached per agent in an in-memory `DashMap` with a
   content-hash check and a 15-minute TTL. Cache hits skip the API call entirely.

3. **Embed the query** (always a fresh call).

4. **Score** each agent by cosine similarity between the query embedding and the agent embedding.

5. **Filter:**
   - If the top score is below 0.2 (no meaningful semantic signal), return **all** agents
     unchanged -- the embeddings aren't helping.
   - Otherwise, return the top-k agents (k = `ROUTER_SHORTLIST_SIZE`, default 10).

### Fallbacks

| Condition | Behavior |
|---|---|
| No `OPENAI_API_KEY` | Skip Stage 1; all agents advance |
| Embedding API error | Skip Stage 1; all agents advance; warning logged |
| Top score < 0.2 | All agents advance (weak signal) |

## Stage 2: Conversation-Aware Reranking

**File:** `oss/orchestrator/src/reranker.rs`

**Purpose:** Re-score the shortlist using conversation context so that an ongoing conversation
biases routing toward the agent that has been handling it.

**When it runs:** Only when conversation history is non-empty. If there is no prior context the
shortlist passes through unchanged.

### How it works

1. **Build context text:**
   ```
   "{history summary}\n{current query}"
   ```
   where `history summary` concatenates prior messages as `"{role}: {content}\n"`.

2. **Embed** the context text using the same vector store (reuses Stage 1's cached agent
   embeddings).

3. **Score** shortlisted agents against the context embedding via cosine similarity.

4. **Sort** by score and return the top-k.

### Fallbacks

| Condition | Behavior |
|---|---|
| Empty history | Stage skipped entirely |
| Embedding error | Shortlist returned unchanged; warning logged |

## Stage 3: LLM Final Selection

**File:** `oss/orchestrator/src/selector.rs`

**Purpose:** Make a definitive, explainable agent selection using an LLM with structured output.

### System prompt

```
You are a routing assistant. Select the best agent to handle the user's query.

Available agents:
- AgentName (ID: <uuid>): <description>
  Skills: skill1: desc1; skill2: desc2
  Tags: tag1, tag2

Select the most specialized agent. If no perfect match, choose closest option.
```

Each candidate is presented with its name, UUID, description, skills, and tags.

### User prompt

If conversation history exists (up to the 5 most recent messages):

```
Conversation history:
user: <message>
assistant: <message>

Current query: <the query>
```

Otherwise just:

```
Current query: <the query>
```

### LLM call

```
model:              ROUTER_MODEL (default "gpt-4o")
temperature:        0.0       (deterministic)
max_tokens:         500
response_format:    json_schema (strict)
```

The structured output schema:

```json
{
  "agent_id":   "string (UUID)",
  "agent_name": "string",
  "reasoning":  "string"
}
```

### Validation

- If the returned `agent_id` is not in the candidate list (hallucination), the first candidate is
  used and the reasoning is updated to explain the fallback.
- If the LLM call itself fails, the first candidate is used and `fallback_used` is set to `true`.

### Fallbacks

| Condition | Behavior |
|---|---|
| LLM call error | First candidate selected; `fallback_used = true` |
| Hallucinated agent_id | First candidate selected; reasoning updated |
| No candidates at all | `RouterError::NoAgentsAvailable` |

## Agent Card Summary

The information the routing engine sees for each agent:

```rust
AgentCardSummary {
    id:          Uuid,
    name:        String,
    description: String,
    skills:      Vec<{ name, description }>,
    tags:        Vec<String>,
}
```

These fields come from the `agents` table and are populated at deploy time (from the agent's A2A
card or the deploy request).

**Tip:** To improve routing accuracy, give your agent a specific `description` and meaningful
`skills` and `tags`. The description and tags feed the embedding in Stages 1-2, and all fields are
shown to the LLM in Stage 3.

## Routing Decision Logging

Every routing decision is logged asynchronously (never blocks the response path):

**Table: `router_request_log`**

| Column | Content |
|---|---|
| `request_id` | Unique request identifier |
| `user_id` | Caller |
| `session_id` | Conversation session |
| `query` | The user's query text |
| `agents_considered` | Count of candidates entering the pipeline |
| `selected_agent_id` | Final agent UUID |
| `selected_agent_name` | Final agent name |
| `selection_reasoning` | LLM's explanation |
| `fallback_used` | Whether any stage fell back |
| `stage1_candidates` | Candidates after Stage 1 |
| `stage2_candidates` | Candidates after Stage 2 |
| `embedding_model` | Model used for embeddings |
| `registry_fetch_ms` | Time to fetch agents from DB |
| `selection_llm_ms` | Time for Stage 3 LLM call |
| `total_latency_ms` | End-to-end routing latency |

Stage 3 token usage is also written to the `token_usage` table (operation type
`router_selection`).

## Observability

- A `traceparent` header is propagated from the client through routing and into the selected
  agent, so the entire request appears as a single distributed trace.
- The routing decision span is emitted by the server and parents the agent's own spans.
- Routing stats are exposed at `GET /api/orchestrator/stats`.

## Configuration Reference

All values are set via environment variables and read through the `Config` struct in
`oss/config/src/lib.rs`.

| Env Var | Default | Description |
|---|---|---|
| `ROUTER_MODEL` | `gpt-4o` | LLM model for Stage 3 selection |
| `EMBEDDING_MODEL` | `text-embedding-3-small` | Embedding model for Stages 1-2 |
| `ROUTER_SHORTLIST_THRESHOLD` | `15` | Skip Stage 1 if fewer agents than this |
| `ROUTER_SHORTLIST_SIZE` | `10` | Top-k returned from Stage 1 |
| `AGENT_CALL_TIMEOUT_SECS` | `600` | Budget for one agent hop — the A2A proxy, the orchestrator's streaming and non-streaming agent calls, and the MAF executor's. Accepts the former name `ROUTER_AGENT_TIMEOUT_SECS` as a fallback |
| `OPENAI_API_KEY` | -- | Required for embeddings and LLM selection |
| `OPENAI_BASE_URL` | OpenAI default | Base URL for embedding/LLM calls |

Session history depth is controlled by `max_router_history_messages` in the config (default 20).

## EE Additions

The enterprise edition supplies its own `RoutingEngine` implementation that wraps
`OssRoutingEngine`. Currently it delegates directly; a planned enhancement
(BACKEND-16) will pre-filter candidates via RBAC `can_access_agent()` checks before Stage 3 so
that the LLM never sees agents the caller cannot reach.

## Direct Agent Path (Bypass Routing)

When `agent_id` is specified in request metadata, routing is skipped entirely:

1. **Resolve** the agent by UUID or name from the DB.
2. **ACL check** -- returns 404 (not 403) if the caller cannot access the agent, to prevent
   enumeration.
3. **Proxy** the A2A JSON-RPC request directly to the agent's container endpoint.
4. **Stream** the SSE response back to the client.

## Multi-Agent Flow (MAF) -- Alternative Orchestration

MAF is a separate orchestration mode (`oss/orchestrator/src/maf/`) for pre-planned multi-step
workflows. Unlike the routing engine which selects **one** agent per query, MAF executes a
**sequence** of steps, each assigned to a specific agent:

1. **Plan** -- LLM generates a step-by-step plan with agent assignments.
2. **Execute** -- Each step runs sequentially: generate prompt -> call agent -> extract results.
3. **Synthesize** -- LLM produces a final response from all step outputs.

MAF is invoked via `POST /api/maf/workflow/{id}/run`, not through the main A2A dispatch path.

## Performance Characteristics

| Component | Typical Latency | Caching |
|---|---|---|
| Agent fetch (DB) | ~10 ms | None |
| Stage 1 (embeddings) | 50-150 ms | Per-agent, 15-min TTL |
| Stage 2 (reranking) | 30-80 ms | Reuses Stage 1 cache |
| Stage 3 (LLM selection) | 100-500 ms | None |
| **Total routing** | **150-750 ms** | Partial |

For small catalogs (< 15 agents), Stage 1 is skipped and total routing latency is dominated by the
Stage 3 LLM call.
