-- Lakehouse tiering core (docs/LAKEHOUSE_TRD.md §4–§6). Postgres-only mutable state.

-- Batch log: one row per append of hot rows to a cold Lance dataset.
CREATE TABLE tier_batches (
    id            BIGSERIAL PRIMARY KEY,
    table_name    TEXT        NOT NULL,
    batch_id      UUID        NOT NULL UNIQUE,  -- also written as a column in every cold row
    row_count     BIGINT      NOT NULL,
    min_ts        TIMESTAMPTZ NOT NULL,
    max_ts        TIMESTAMPTZ NOT NULL,
    lance_version BIGINT,                       -- set once the Lance commit succeeds
    state         TEXT        NOT NULL CHECK (state IN ('writing', 'live')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_tier_batches_table_state ON tier_batches (table_name, state);

-- GDPR erasures not yet complete across hot + cold (§9). Row deleted on success;
-- the tiering worker retries the rest. No FK: the user row is already gone.
CREATE TABLE erasure_requests (
    user_id         UUID        PRIMARY KEY,
    requested_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts        INT         NOT NULL DEFAULT 0,
    last_attempt_at TIMESTAMPTZ,
    last_error      TEXT
);

-- Tiering deletes token_usage rows; the ON DELETE SET NULL FK would blank this id.
-- Keep the id, drop the constraint.
ALTER TABLE router_request_log
    DROP CONSTRAINT IF EXISTS router_request_log_selection_token_usage_id_fkey;
-- storage: metric (token_usage keeps its rows: renamed, then drained to the cold tier)
-- token_usage is retired. trace_usage (0013, 0080) is the single spend + trace table:
-- the router upserts one row per (trace_id, agent_name) live, and per-call router metadata
-- (brevity/minimal_code arms, key_source, operation_type) lives in trace_usage.metadata.
-- Drop every object built on the raw call log. Nothing writes the log any more.
--
-- The log itself keeps its history: it is renamed, not dropped, and the tiering job
-- copies its rows to the cold `token_usage` dataset and drops it once empty
-- (EventStore::tier_legacy_usage_batch). On a fresh install it is empty and goes on
-- the first tiering run.

DROP VIEW IF EXISTS usage_spend;
DROP MATERIALIZED VIEW IF EXISTS token_usage_daily;
DROP TABLE IF EXISTS usage_flows;
DROP TABLE IF EXISTS usage_daily;
ALTER TABLE IF EXISTS token_usage RENAME TO token_usage_legacy;
-- cold: compatible (trace_usage has no cold rows before this migration; tiering starts here)
-- trace_usage is the single spend + trace table: one row per (trace_id, agent_name) from the
-- materializer (source 'agent'), direct:<id> rows for coding agents
-- (source 'coding'), and ACP sessions (source 'acp'). Budgets, dashboards and FinOps
-- read spend only from trace_usage / trace_spend (token_usage is dropped in 0079).
--
-- trace_usage → Lance after 90 days. Before delete, rows roll into trace_usage_daily
-- (per day, for date-range dashboards) and trace_totals (per trace, for id-keyed lookups:
-- session lists, subagent totals, maf steps, savings by_session). Kept forever.
-- Nil UUID / '' stand for NULL so every key column can sit in the primary key.

ALTER TABLE trace_usage
    ADD COLUMN source TEXT NOT NULL DEFAULT 'agent'
        CHECK (source IN ('agent', 'coding', 'acp')),
    -- Who wrote the spend columns. 'router' rows are upserted live per LLM call and
    -- the Tempo materializer never overwrites their spend; 'tempo' is the fallback
    -- for calls that skipped the router (own provider key).
    ADD COLUMN priced_by TEXT NOT NULL DEFAULT 'tempo'
        CHECK (priced_by IN ('router', 'tempo')),
    -- Per-call router metadata merged per trace: brevity / minimal_code arms,
    -- key_source, operation_type. Replaces token_usage.metadata.
    ADD COLUMN metadata JSONB NOT NULL DEFAULT '{}',
    -- The router's own share of the row, kept apart from the visible columns so a
    -- trace that mixes routed and own-key calls keeps both: the router adds each
    -- call here and to the visible column, and the Tempo materializer (whose
    -- aggregate covers routed and own-key calls alike) sets the visible column to
    -- GREATEST(tempo, router), i.e. router + whatever Tempo saw beyond it. Hot only:
    -- the cold copy and the rollups carry the visible columns.
    ADD COLUMN router_input_tokens          BIGINT           NOT NULL DEFAULT 0,
    ADD COLUMN router_output_tokens         BIGINT           NOT NULL DEFAULT 0,
    ADD COLUMN router_cache_read_tokens     BIGINT           NOT NULL DEFAULT 0,
    ADD COLUMN router_cache_creation_tokens BIGINT           NOT NULL DEFAULT 0,
    ADD COLUMN router_cost_usd              DOUBLE PRECISION NOT NULL DEFAULT 0,
    ADD COLUMN router_prompt_cost_usd       DOUBLE PRECISION NOT NULL DEFAULT 0,
    ADD COLUMN router_completion_cost_usd   DOUBLE PRECISION NOT NULL DEFAULT 0;

-- Existing direct:<id> rows were priced by the router (coding agents).
UPDATE trace_usage SET source = 'coding', priced_by = 'router' WHERE trace_id LIKE 'direct:%';
-- Every router-priced row so far holds only router spend.
UPDATE trace_usage SET
    router_input_tokens          = COALESCE(input_tokens, 0),
    router_output_tokens         = COALESCE(output_tokens, 0),
    router_cache_read_tokens     = COALESCE(cache_read_tokens, 0),
    router_cache_creation_tokens = COALESCE(cache_creation_tokens, 0),
    router_cost_usd              = COALESCE(cost_usd, 0),
    router_prompt_cost_usd       = COALESCE(prompt_cost_usd, 0),
    router_completion_cost_usd   = COALESCE(completion_cost_usd, 0)
WHERE priced_by = 'router';

CREATE INDEX idx_trace_usage_user_source_started ON trace_usage (user_id, source, started_at);

CREATE TABLE trace_usage_daily (
    day                   DATE             NOT NULL,
    user_id               UUID             NOT NULL,
    agent_id              UUID             NOT NULL,
    agent_name            TEXT             NOT NULL,
    source                TEXT             NOT NULL DEFAULT 'agent',
    model                 TEXT             NOT NULL,
    provider              TEXT             NOT NULL,
    traces                BIGINT           NOT NULL DEFAULT 0,
    input_tokens          BIGINT           NOT NULL DEFAULT 0,
    output_tokens         BIGINT           NOT NULL DEFAULT 0,
    cache_read_tokens     BIGINT           NOT NULL DEFAULT 0,
    cache_creation_tokens BIGINT           NOT NULL DEFAULT 0,
    cost_usd              DOUBLE PRECISION NOT NULL DEFAULT 0,
    prompt_cost_usd       DOUBLE PRECISION NOT NULL DEFAULT 0,
    completion_cost_usd   DOUBLE PRECISION NOT NULL DEFAULT 0,
    latency_sum           BIGINT           NOT NULL DEFAULT 0,
    latency_count         BIGINT           NOT NULL DEFAULT 0,
    tool_call_count       BIGINT           NOT NULL DEFAULT 0,
    error_count           BIGINT           NOT NULL DEFAULT 0,
    PRIMARY KEY (day, user_id, agent_id, agent_name, source, model, provider)
);
CREATE INDEX idx_trace_usage_daily_user_day ON trace_usage_daily (user_id, day);
CREATE INDEX idx_trace_usage_daily_agent_day ON trace_usage_daily (agent_name, day);

-- Per-trace totals, additive across batches.
CREATE TABLE trace_totals (
    trace_id              TEXT             NOT NULL,
    session_id            TEXT,
    user_id               UUID             NOT NULL,
    first_at              TIMESTAMPTZ      NOT NULL,
    tokens                BIGINT           NOT NULL DEFAULT 0,
    cost_usd              DOUBLE PRECISION NOT NULL DEFAULT 0,
    errors                BIGINT           NOT NULL DEFAULT 0,
    max_latency_ms        BIGINT,
    input_tokens          BIGINT           NOT NULL DEFAULT 0,
    output_tokens         BIGINT           NOT NULL DEFAULT 0,
    cache_read_tokens     BIGINT           NOT NULL DEFAULT 0,
    cache_creation_tokens BIGINT           NOT NULL DEFAULT 0,
    model                 TEXT,            -- set when the trace used a single model
    source                TEXT             NOT NULL DEFAULT 'agent',
    -- The trace's first priced call (provider, model) and first known agent, kept so
    -- readers that price or attribute by trace still resolve once the rows are tiered.
    first_provider        TEXT,
    first_model           TEXT,
    first_agent_id        UUID,
    -- trace_usage rows (one per agent) rolled in, so per-turn call counts match
    -- what the hot rows showed.
    row_count             BIGINT           NOT NULL DEFAULT 0,
    PRIMARY KEY (trace_id, user_id)
);
CREATE INDEX idx_trace_totals_session ON trace_totals (session_id) WHERE session_id IS NOT NULL;
CREATE INDEX idx_trace_totals_user ON trace_totals (user_id);

-- A trace's calls as price/session/agent lookups need them, hot or tiered: one row
-- per hot call plus one per tiered (trace, user). Lookups filter on trace_id and
-- take the earliest row, so the hot side and the frozen side agree.
CREATE VIEW trace_first_calls AS
    SELECT trace_id, started_at, provider, model, agent_id, session_id
      FROM trace_usage
    UNION ALL
    SELECT trace_id, first_at, first_provider, first_model, first_agent_id, session_id
      FROM trace_totals;

-- Hot trace_usage rows + tiered days. Each row lives on exactly one side.
-- The only spend source for budgets, dashboards and FinOps.
CREATE VIEW trace_spend AS
    SELECT started_at AS ts, user_id, agent_id, agent_name, model, provider,
           1::bigint AS traces, input_tokens, output_tokens, cache_read_tokens,
           cache_creation_tokens, cost_usd, prompt_cost_usd, completion_cost_usd,
           COALESCE(latency_ms, 0) AS latency_sum,
           (latency_ms IS NOT NULL)::int::bigint AS latency_count,
           tool_call_count, COALESCE(error_count, 0)::bigint AS error_count,
           source
    FROM trace_usage
    UNION ALL
    SELECT (day::timestamp AT TIME ZONE 'UTC'),
           NULLIF(user_id, '00000000-0000-0000-0000-000000000000'::uuid),
           NULLIF(agent_id, '00000000-0000-0000-0000-000000000000'::uuid),
           agent_name, NULLIF(model, ''), NULLIF(provider, ''),
           traces, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
           cost_usd, prompt_cost_usd, completion_cost_usd, latency_sum, latency_count,
           tool_call_count, error_count,
           source
    FROM trace_usage_daily;
-- token_savings and context_reports → Lance after 90 days, rolled up first. Kept forever.
-- Cost and agent are resolved once, at tiering time, and frozen in the rollups.
-- Nil UUID / '' stand for NULL so every key column can sit in the primary key.

-- ── token_savings ────────────────────────────────────────────────────────────────────
-- Rows without a model are priced at their flow's first LLM call (trace_first_calls,
-- trace_id = flow_id: hot trace_usage or its tiered trace_totals row); rows with a
-- model keep their own saved_cost_usd.
CREATE TABLE token_savings_daily (
    day                 DATE             NOT NULL,
    user_id             UUID             NOT NULL,
    agent_id            UUID             NOT NULL,
    layer               TEXT             NOT NULL,
    program             TEXT             NOT NULL,
    provider            TEXT             NOT NULL,
    model               TEXT             NOT NULL,
    context_tier        TEXT             NOT NULL,
    rows                BIGINT           NOT NULL DEFAULT 0,
    saved_input_tokens  BIGINT           NOT NULL DEFAULT 0,
    saved_output_tokens BIGINT           NOT NULL DEFAULT 0,
    calibrated_tokens   BIGINT           NOT NULL DEFAULT 0,  -- saved_input where NOT token_estimated
    uncalibrated_tokens BIGINT           NOT NULL DEFAULT 0,
    raw_cost_usd        DOUBLE PRECISION NOT NULL DEFAULT 0,  -- sum(saved_cost_usd) as written
    priced_cost_usd     DOUBLE PRECISION NOT NULL DEFAULT 0,  -- PRICED_COST, frozen
    unpriced_tokens     BIGINT           NOT NULL DEFAULT 0,  -- UNPRICED_TOKENS, frozen
    PRIMARY KEY (day, user_id, agent_id, layer, program, provider, model, context_tier)
);
CREATE INDEX idx_token_savings_daily_user_day ON token_savings_daily (user_id, day);

-- Per-flow savings, so savings by_session keeps tiered sessions. Additive across batches.
CREATE TABLE savings_flows (
    flow_id             TEXT             NOT NULL,
    user_id             UUID             NOT NULL,
    agent_id            UUID             NOT NULL,
    session_id          TEXT,                                 -- resolved via trace_first_calls
    first_at            TIMESTAMPTZ      NOT NULL,
    saved_input_tokens  BIGINT           NOT NULL DEFAULT 0,
    saved_output_tokens BIGINT           NOT NULL DEFAULT 0,
    priced_cost_usd     DOUBLE PRECISION NOT NULL DEFAULT 0,
    PRIMARY KEY (flow_id, user_id, agent_id)
);
CREATE INDEX idx_savings_flows_user ON savings_flows (user_id);
CREATE INDEX idx_savings_flows_first_at ON savings_flows (first_at);

-- Hot rows priced live + tiered days. Rollup rows carry no session_id.
CREATE VIEW savings_spend AS
    SELECT ts.created_at AS ts, ts.user_id, ts.agent_id, ts.provider, ts.model,
           ts.layer, ts.program, ts.context_tier, ts.session_id,
           1::bigint AS rows,
           ts.saved_input_tokens, ts.saved_output_tokens,
           CASE WHEN NOT ts.token_estimated THEN ts.saved_input_tokens ELSE 0 END::bigint AS calibrated_tokens,
           CASE WHEN ts.token_estimated THEN ts.saved_input_tokens ELSE 0 END::bigint AS uncalibrated_tokens,
           ts.saved_cost_usd AS raw_cost_usd,
           (CASE WHEN ts.model IS NULL THEN ts.saved_input_tokens * COALESCE(price.rate, 0)
                 ELSE ts.saved_cost_usd END)::float8 AS priced_cost_usd,
           (CASE WHEN ts.model IS NULL AND price.rate IS NULL
                 THEN ts.saved_input_tokens + ts.saved_output_tokens ELSE 0 END)::bigint AS unpriced_tokens
      FROM token_savings ts
      LEFT JOIN LATERAL (
          SELECT (mp.input_price_per_1m / 1000000.0)::FLOAT8 AS rate
            FROM trace_first_calls tu
            JOIN model_pricing mp
              ON mp.provider = tu.provider AND mp.model = tu.model
             AND mp.effective_from <= ts.created_at
             AND (mp.effective_until IS NULL OR mp.effective_until > ts.created_at)
           WHERE tu.trace_id = ts.flow_id
           ORDER BY tu.started_at ASC, mp.effective_from DESC
           LIMIT 1
      ) price ON true
    UNION ALL
    SELECT (day::timestamp AT TIME ZONE 'UTC'), user_id,
           NULLIF(agent_id, '00000000-0000-0000-0000-000000000000'::uuid),
           NULLIF(provider, ''), NULLIF(model, ''), layer, program, NULLIF(context_tier, ''),
           NULL::text,
           rows, saved_input_tokens, saved_output_tokens, calibrated_tokens, uncalibrated_tokens,
           raw_cost_usd, priced_cost_usd, unpriced_tokens
      FROM token_savings_daily;

-- ── context_reports ──────────────────────────────────────────────────────────────────
-- Agent (cr.agent_id → session_traces → trace_first_calls) and request price frozen at tiering.
CREATE TABLE context_reports_daily (
    day              DATE             NOT NULL,
    user_id          UUID             NOT NULL,
    agent_id         UUID             NOT NULL,  -- resolved agent; nil = none
    strategy         TEXT             NOT NULL,
    level            TEXT             NOT NULL,
    fallback         BOOLEAN          NOT NULL,
    reports          BIGINT           NOT NULL DEFAULT 0,
    priced_reports   BIGINT           NOT NULL DEFAULT 0,
    pool_tokens      BIGINT           NOT NULL DEFAULT 0,
    kept_tokens      BIGINT           NOT NULL DEFAULT 0,
    messages_dropped BIGINT           NOT NULL DEFAULT 0,
    compressed_bytes BIGINT           NOT NULL DEFAULT 0,
    pool_cost        DOUBLE PRECISION NOT NULL DEFAULT 0,
    sent_cost        DOUBLE PRECISION NOT NULL DEFAULT 0,
    PRIMARY KEY (day, user_id, agent_id, strategy, level, fallback)
);
CREATE INDEX idx_context_reports_daily_user_day ON context_reports_daily (user_id, day);

-- Hot rows (agent/price resolved live by the reader) + tiered days (frozen).
-- Rollup rows carry no trace/span/session; frozen_* are NULL on hot rows.
CREATE VIEW context_reports_spend AS
    SELECT created_at, user_id, agent_id, session_id, trace_id, span_id,
           1::bigint AS reports,
           pool_messages::bigint AS pool_messages, kept_messages::bigint AS kept_messages,
           pool_tokens, kept_tokens, compressed_bytes_saved,
           NULL::bigint AS frozen_priced, NULL::float8 AS frozen_pool_cost, NULL::float8 AS frozen_sent_cost
      FROM context_reports
    UNION ALL
    SELECT (day::timestamp AT TIME ZONE 'UTC'), user_id,
           NULLIF(agent_id, '00000000-0000-0000-0000-000000000000'::uuid),
           NULL::text, NULL::text, NULL::text,
           reports, messages_dropped, 0::bigint, pool_tokens, kept_tokens, compressed_bytes,
           priced_reports, pool_cost, sent_cost
      FROM context_reports_daily;
-- One row per tool call, so tool calls outlive Tempo retention (docs/LAKEHOUSE_TRD.md, tool_calls).
-- Hot in Postgres, tiered to Lance like trace_usage. Producers: coding-agent ingest (ACP turns)
-- and the trace materializer (agent `execute_tool` / `call_tool` spans).
-- Keyed (trace_id, call_id) so a re-ingested turn or re-read trace replaces, never duplicates.
CREATE TABLE tool_calls (
    trace_id     TEXT        NOT NULL,
    call_id      TEXT        NOT NULL,
    user_id      UUID        REFERENCES users(id) ON DELETE CASCADE,
    agent_id     UUID,
    agent_name   TEXT        NOT NULL,
    session_id   TEXT,
    tool_name    TEXT        NOT NULL,
    tool_kind    TEXT,
    status       TEXT        NOT NULL,
    error        TEXT,
    -- Bodies: kept inline here; the tiering job moves bodies above a size limit to object storage.
    arguments    JSONB,
    output       JSONB,
    duration_ms  BIGINT,
    started_at   TIMESTAMPTZ NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (trace_id, call_id)
);
CREATE INDEX idx_tool_calls_user_time ON tool_calls (user_id, started_at DESC);
CREATE INDEX idx_tool_calls_session ON tool_calls (session_id);
CREATE INDEX idx_tool_calls_started ON tool_calls (started_at);
-- Phase 3 (docs/LAKEHOUSE_TRD.md §8.1): messages of idle sessions move to Lance.
-- cold: compatible
-- `cold_since` is set by the tiering job when the session's first batch goes cold;
-- NULL means every message is still in Postgres, so `load_session` skips Lance.
ALTER TABLE chat_sessions ADD COLUMN cold_since TIMESTAMPTZ;
-- A deleted chat's tiered messages, pending removal from the cold Lance datasets.
-- Written in the same transaction as the `chat_sessions` delete; the row goes once
-- every cold copy is gone, and the tiering worker retries the rest.
-- `batch_watermark` is the newest `tier_batches.id` at deletion: a session re-created
-- under the same id (coding sessions, client-chosen A2A context ids) is tiered in later
-- batches, which the erase never touches.
CREATE TABLE session_erasure_requests (
    session_id      TEXT        PRIMARY KEY,
    batch_watermark BIGINT      NOT NULL,
    requested_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts        INT         NOT NULL DEFAULT 0,
    last_attempt_at TIMESTAMPTZ,
    last_error      TEXT
);
-- =============================================================================
-- flows.session_id NOT NULL, step 1 of 2 (docs/FLOWS_LAKEHOUSE_PLAN.md issues
-- 1, 2, 8): backfill + NOT VALID check + parent_flow_id.
--
-- Split across 0079/0080 so no statement holds ACCESS EXCLUSIVE while
-- scanning `flows` (sqlx wraps each file in one transaction, so a lock taken
-- early is held until the file commits):
--   0079  UPDATE touches only NULL rows (0058 backfilled most) under row locks;
--         the DDL runs last and is catalog-only, so ACCESS EXCLUSIVE is brief.
--   0080  no-transaction file: VALIDATE under SHARE UPDATE EXCLUSIVE (reads
--         and writes continue), then SET NOT NULL reuses the validated
--         check (PG 12+): no scan.
-- lock_timeout makes DDL fail fast instead of queueing behind long
-- transactions (a queued ACCESS EXCLUSIVE request blocks every later query).
-- No FK to chat_sessions: a flow outlives a deleted session (0058).
-- =============================================================================

SET LOCAL lock_timeout = '5s';

-- Priority: metadata context_id, then the legacy session_traces mapping
-- (oldest row, via idx_session_traces_trace), then the flow as its own session.
UPDATE flows f
   SET session_id = COALESCE(
           NULLIF(f.metadata->>'context_id', ''),
           (SELECT st.session_id
              FROM session_traces st
             WHERE st.trace_id = f.flow_id
             ORDER BY st.created_at ASC
             LIMIT 1),
           f.flow_id)
 WHERE f.session_id IS NULL;

-- Enforced for new rows immediately; existing rows are checked in 0080.
ALTER TABLE flows
    ADD CONSTRAINT flows_session_id_not_null CHECK (session_id IS NOT NULL) NOT VALID;

-- HITL resume starts a fresh trace; link it to the flow it resumes. Nullable,
-- no default: catalog-only. No FK: the parent may already be tiered to Lance.
ALTER TABLE flows ADD COLUMN parent_flow_id TEXT;
-- Flows tiering keeps a slim hot row so SQL joins (ownership, session lists,
-- budgets, MCP identity) keep working and `flow_steps` / `flow_participants`
-- never cascade-delete. Tiering copies the full row to Lance, clears the heavy
-- columns and stamps `tiered_at`. Nullable, no default: catalog-only.
SET LOCAL lock_timeout = '5s';
ALTER TABLE flows ADD COLUMN tiered_at TIMESTAMPTZ;
