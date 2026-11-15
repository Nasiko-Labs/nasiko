# Routing Engine

How the platform picks an agent for a task: the 3-stage routing engine, and how it relates to the
chat orchestrator.

## Overview

The **routing engine** (`RoutingEngine` / `OssRoutingEngine`, `orchestrator/src/engine.rs`) is a
**3-stage semantic routing pipeline** that selects one agent for a piece of text. It progressively
narrows a large agent catalog down to a single agent using embeddings, conversation context, and
an LLM final decision.

Today its caller is **MAF**: when a workflow is created or updated with a step that names no
agent, the step's task description is routed through `RoutingEngine::route` to auto-assign one
(`route_with_access_check` in `server/src/maf.rs`). The chat entry point does **not** use it --
see [Chat path: the ReAct orchestrator](#chat-path-the-react-orchestrator) below.

```
  Task description (MAF step)
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
  Selected agent --> assigned to the step
```

Infrastructure faults degrade gracefully: if embeddings are unavailable, if history is empty, or
if the LLM call errors, the pipeline still returns an agent instead of failing. The one exception
is an explicit refusal by the operator's `RoutingPolicy` (see
[Routing policy](#routing-policy)), which is returned as an error with no fallback pick.

## Chat path: the ReAct orchestrator

**File:** `server/src/router/a2a_dispatch.rs`

`POST /api/orchestrator/a2a` inspects `metadata.agent_id` in the incoming JSON-RPC request:

| `agent_id` value | Behavior |
|---|---|
| absent or `"orchestrator"` | **Orchestrator path** -- the ReAct orchestrator (`orchestrator_stream`) |
| any other value (name or UUID) | **Direct path** -- resolves the agent, checks ACL, forwards directly |

For the orchestrator path, `orchestrator_stream`:

1. Fetches conversation history with `context_selection::fetch_for_user`, using the caller's
   stored context strategy and budget level (`ContextTiers`; deployment values `PACMS_*` and
   `CONTEXT_K_*` in `config/src/lib.rs`), and enriches the current query with it.
2. Loads the routable agents (`AgentSelector::fetch_active_agents`: status `running` or `parked`,
   not deleted, not internal) and keeps those the caller may access.
3. Drops harness agents (a routed turn cannot drive their sessions) and any agents that share a
   display name (a call could not tell them apart).
4. Hands the remaining agents, each as an A2A tool, to a ReAct loop (`react-agent` crate, up to
   10 turns, model `OPENAI_MODEL` or the LLM router's default). The model may call **several**
   agents, call one repeatedly, or **answer directly** without calling any.
5. Each agent call the model makes is resolved, woken if parked, and checked (ACL, flow guard)
   by `CpCallGuard` before it is forwarded; the turn is streamed back as SSE.

The 3-stage pipeline below is not involved on this path.

## Access Control (Pre-filter)

Before any routing stage, agents are filtered by access control. An agent is accessible if **any**
of these hold:

- The caller owns the agent (`agents.owner_id`).
- The agent is public (`agents.is_public`).
- An `agent_grants` row exists for the caller (a user grant).
- The caller is a superuser (skips all checks).
- **(Enterprise edition)** Organization and team grants.

Soft-deleted agents are always excluded. The `agent_acl` allowlist table additionally restricts
which agents can call which other agents during inter-agent routing.

**File:** `server/src/acl.rs` (the enterprise edition layers its grant matrix on top of this
through the `AuthService` trait).

## Stage 1: Vector Semantic Shortlist

**File:** `orchestrator/src/vector_store.rs`

**Purpose:** Quickly narrow a large catalog (tens or hundreds of agents) to a small candidate set
using embedding similarity.

**When it runs:** Only when the number of accessible agents >= `ROUTER_SHORTLIST_THRESHOLD`
(default 15). For smaller catalogs the stage is skipped and all agents advance.

### How it works

1. **Build agent text** for embedding:
   ```
   "{name} {description} {tags joined by space}"
   ```

2. **Embed agents** via the platform LLM backend (the LLM router, so any configured provider
   serves them), or directly against OpenAI when the user has saved their own key (default model
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
| No backend can embed with the configured model | Skip Stage 1; all agents advance |
| Embedding API error | Skip Stage 1; all agents advance; warning logged |
| Top score < 0.2 | All agents advance (weak signal) |

## Stage 2: Conversation-Aware Reranking

**File:** `orchestrator/src/reranker.rs`

**Purpose:** Re-score the shortlist using conversation context so that an ongoing conversation
biases routing toward the agent that has been handling it.

**When it runs:** Only when conversation history is non-empty. If there is no prior context the
shortlist keeps its order but is still truncated to the top k (`ROUTER_SHORTLIST_SIZE`). MAF
routes each step under a fresh session id, so on that path history is empty.

### How it works

1. **Build context text:**
   ```
   "{history summary} {current query}"
   ```
   where `history summary` is the prior messages as `"{role}: {content}"` lines, joined to the
   query by a space.

2. **Embed** the context text using the same vector store (reuses Stage 1's cached agent
   embeddings).

3. **Score** shortlisted agents against the context embedding via cosine similarity.

4. **Sort** by score and return the top-k.

### Fallbacks

| Condition | Behavior |
|---|---|
| Empty history | No embedding call; shortlist truncated to the top k in its existing order |
| Embedding error | Shortlist truncated to the top k unchanged; warning logged |

## Stage 3: LLM Final Selection

**File:** `orchestrator/src/selector.rs`

**Purpose:** Make a definitive, explainable agent selection using an LLM with structured output.

### System prompt

```
You are a routing assistant. Select the best agent to handle the user's query.

Available agents:
- AgentName (ID: <uuid>): <description>
  Skills: skill1: desc1; skill2: desc2
  Tags: tag1, tag2

Select the most specialized agent. If no perfect match, choose the closest option.
```

Each candidate is presented with its name, UUID, description, skills, and tags. A `RoutingPolicy`
may add a prefix to the first line and replace the closing instruction.

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
model:              ROUTER_MODEL (else DEFAULT_MODEL, else the LLM router's provider default)
temperature:        0.0       (deterministic)
max_tokens:         2048
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

- If a `RoutingPolicy` is in force, it judges the raw selection **first**; a selection it refuses
  is returned as `RouterError::PolicyRefused` with no fallback pick (see
  [Routing policy](#routing-policy)).
- If the returned `agent_id` is not in the candidate list (hallucination), the first candidate is
  used, the reasoning is updated to explain the fallback, and `fallback_used` is set to `true`.
- If the LLM call itself fails, the first candidate is used and `fallback_used` is set to `true`.

### Fallbacks

| Condition | Behavior |
|---|---|
| LLM call error | First candidate selected; `fallback_used = true` |
| Hallucinated agent_id | First candidate selected; reasoning updated; `fallback_used = true` |
| Refused by the `RoutingPolicy` | `RouterError::PolicyRefused`; no agent selected |
| No candidates at all | `RouterError::NoAgentsAvailable` |

### Routing policy

`RoutingEngine::route` takes an optional `RoutingPolicy` (`orchestrator/src/policy.rs`). With
none (the open-source default) the selector behaves as above. A policy can add fields to the
structured-output schema, reword the prompt, and reject a selection; a rejection is a decision,
not a fault, so it propagates as `RouterError::PolicyRefused { reason }` instead of falling back
to the first candidate. It is still logged to `router_request_log` (`success = false`) and its
token usage is still recorded. MAF surfaces the refusal to the caller rather than assigning the
step from its catalog fallback.

## Agent Card Summary

The information the routing engine sees for each agent:

```rust
AgentCardSummary {
    id:          Uuid,
    name:        String,
    description: String,
    skills:      Vec<{ name, description, examples }>,
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

Stage 3 token usage is recorded in the `trace_usage` table (operation type
`router_selection`): by the LLM router when the call goes through the platform backend, or by
the engine itself when it calls a provider directly with the user's saved key.

## Observability

- A `traceparent` header is propagated from the client through routing and into the selected
  agent, so the entire request appears as a single distributed trace.
- The routing decision span is emitted by the server and parents the agent's own spans.
- Routing stats are exposed at `GET /api/orchestrator/stats`.

## Configuration Reference

All values are set via environment variables and read through the `Config` struct in
`config/src/lib.rs`.

| Env Var | Default | Description |
|---|---|---|
| `ROUTER_MODEL` | `DEFAULT_MODEL`, else empty | LLM model for Stage 3 selection; empty lets the LLM router use its provider's default model |
| `EMBEDDING_MODEL` | `text-embedding-3-small` | Embedding model for Stages 1-2 |
| `ROUTER_SHORTLIST_THRESHOLD` | `15` | Skip Stage 1 if fewer agents than this |
| `ROUTER_SHORTLIST_SIZE` | `10` | Top-k returned from Stage 1 |
| `AGENT_CALL_TIMEOUT_SECS` | `600` | Budget for one agent hop — the A2A proxy, the orchestrator's streaming and non-streaming agent calls, and the MAF executor's. Accepts the former name `ROUTER_AGENT_TIMEOUT_SECS` as a fallback |
| `OPENAI_API_KEY` | -- | Optional. Embeddings and selection go through the platform LLM backend (the LLM router, any configured provider); this is one provider credential it can use |
| `OPENAI_BASE_URL` | OpenAI default | Base URL for direct calls made with a user's saved OpenAI key |

How much conversation history is read is not a fixed message count: `context_selection::fetch_for_user`
selects it per the caller's stored context strategy and budget level (`ContextTiers`), whose
deployment values are `PACMS_BUDGET_LOW/MEDIUM/HIGH`, `CONTEXT_K_LOW/MEDIUM/HIGH`,
`PACMS_HISTORY_POOL_SIZE` and `PACMS_HISTORY_MANDATORY_RECENT`.

## EE Additions

The enterprise edition can supply its own `RoutingEngine` implementation that wraps
`OssRoutingEngine`.

## Direct Agent Path (Bypass Routing)

When `agent_id` is specified in request metadata, the orchestrator is skipped entirely:

1. **Resolve** the agent by UUID or name from the DB.
2. **ACL check** -- returns 404 (not 403) if the caller cannot access the agent, to prevent
   enumeration.
3. **Session ownership** -- a `contextId` (or `metadata.session_id`) that belongs to another
   user is refused (403); superusers keep access.
4. **Code-driving agents** -- a harness agent is diverted to a harness session turn, and an ACP
   coding agent to a coding turn; neither has an HTTP endpoint to forward to.
5. **Wake** -- a parked agent is woken first (`AgentWake`); if it cannot be woken the request
   gets 503.
6. **Forward** the A2A JSON-RPC request to the agent's endpoint and **stream** the SSE response
   back to the client.

## Multi-Agent Flow (MAF) -- Alternative Orchestration

MAF is a separate orchestration mode (`orchestrator/src/maf/`) for pre-planned multi-step
workflows, and the routing engine's caller (to auto-assign a step that names no agent). Unlike
the routing engine, which selects **one** agent per query, MAF executes a
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
