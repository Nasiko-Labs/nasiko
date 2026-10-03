-- Per-layer savings ledger: what each token-optimization layer removed, on the same
-- dimensions the FinOps dashboard already filters by.
--
-- Why a table and not `token_usage.metadata`. The compression block has lived in that
-- JSONB since IP-1 shipped, and it has never reached a dashboard: FinOps aggregates
-- `trace_usage`, which has no `metadata` column and no join to `token_usage`
-- (ee/docs/CAVEMAN_TOKEN_OPTIMIZATION_PRD_TRD.md §12.2 records this as goal G4 unmet).
-- A GIN-indexed JSONB path is also the wrong shape for the question being asked: one call
-- carries several layers at once, so the aggregate needs GROUP BY layer, not a bag.
--
-- Why not columns on `trace_usage`. Three of the layers (ReAct tool results, session
-- history, context selection) run in the orchestrator, before and around calls rather than
-- inside one — history is fetched before any routing decision, so the chat session carries
-- `agent_id = NULL` and has no trace_usage row of its own. And trace_usage is materialized
-- from Tempo on a cursor; writing into it means racing the materializer.
CREATE TABLE token_savings (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Dimensions. Deliberately the same set `/finops/dashboard` filters by, so the savings
    -- aggregate answers the same question as the spend aggregate sitting beside it.
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    agent_id        UUID REFERENCES agents(id) ON DELETE SET NULL,
    -- W3C traceparent trace-id. Same value as `token_usage.session_id` and
    -- `trace_usage.trace_id`, which is what makes this table joinable to both.
    flow_id         TEXT,
    -- A2A contextId — the chat session, for the session-scoped rollup. NOT the same key as
    -- flow_id: a chat session spans many flows.
    session_id      TEXT,
    provider        TEXT,
    model           TEXT,

    -- Which layer saved this. Stable strings, spelled out rather than derived from a Rust
    -- variant name, because changing one changes a queryable value.
    layer           TEXT NOT NULL CHECK (layer IN (
                        'compress_payload',      -- IP-1, llm-router egress
                        'compress_tool_result',  -- IP-3, ReAct context
                        'compress_history',      -- IP-4, session history read
                        'context_selection',     -- PACMS/TopK/LastK budget effect
                        'brevity',               -- IP-2, factor-derived
                        'minimal_code',          -- Ponytail ladder, factor-derived
                        'prompt_comments')),
    -- Grouping for the dashboard's category rows: 'caveman' | 'pacms' | 'ponytail' |
    -- 'prompt_comments'. Denormalized from `layer` on purpose — the mapping is a product
    -- decision about how these are named to users, not a property of the layer.
    program         TEXT NOT NULL,

    -- Ground truth for the measured layers. NULL on factor-derived rows, which have no bytes.
    bytes_before    BIGINT,
    bytes_after     BIGINT,

    -- Signed. Context selection can legitimately come out negative (a `high` budget tier can
    -- admit more than the 20-message baseline would have), and clamping that to zero would
    -- make this a marketing number rather than a measurement.
    saved_input_tokens  BIGINT NOT NULL DEFAULT 0,
    saved_output_tokens BIGINT NOT NULL DEFAULT 0,
    saved_cost_usd      DOUBLE PRECISION NOT NULL DEFAULT 0,

    -- Provenance. A savings row whose method is not stated cannot be audited.
    method          TEXT NOT NULL CHECK (method IN ('measured_bytes', 'measured_tokens')),
    -- False when chars-per-token was calibrated from this call's own reported usage; true
    -- when it fell back to the shared divisor. The headline percentage divides an estimated
    -- numerator by a provider-reported denominator, so the error does not cancel — this
    -- column is what lets the API report how much of a window was calibrated.
    token_estimated BOOLEAN NOT NULL DEFAULT true,
    -- The user's PACMS budget tier at call time ('low'|'medium'|'high'). Only set on
    -- `context_selection` rows, where the tier is the variable that moves the number.
    context_tier    TEXT,

    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Window scan: every scope starts here.
CREATE INDEX idx_token_savings_window  ON token_savings (created_at DESC);
-- scope=agent, and the per-agent filter shared with the spend aggregate.
CREATE INDEX idx_token_savings_agent   ON token_savings (agent_id, created_at DESC) WHERE agent_id IS NOT NULL;
CREATE INDEX idx_token_savings_user    ON token_savings (user_id, created_at DESC);
-- Category rows.
CREATE INDEX idx_token_savings_layer   ON token_savings (layer, created_at DESC);
-- scope=session. Mirrors idx_trace_usage_session so both tables answer a session query alike.
CREATE INDEX idx_token_savings_session ON token_savings (session_id, created_at DESC) WHERE session_id IS NOT NULL;
-- Join back to token_usage / trace_usage for a single flow.
CREATE INDEX idx_token_savings_flow    ON token_savings (flow_id) WHERE flow_id IS NOT NULL;
