-- no-transaction
-- The idle reapers ask whether a model call was attributed to an agent recently:
-- `trace_usage.agent_id = $1 AND materialized_at > <window start>`. The router upserts one row per
-- flow and moves only `materialized_at` on every further call, so `started_at` (the existing
-- `idx_trace_usage_agent_id`) cannot bound the scan, and without this index the check reads the
-- agent's whole history. Keyed by agent, then the instant a call last landed, the check reads only
-- the rows inside the window.
-- CONCURRENTLY so building it never blocks the router's live upserts. One statement per file,
-- because a no-transaction migration runs outside any transaction.
-- If the build fails, Postgres keeps an INVALID index under this name and IF NOT EXISTS would skip
-- it on the next start: run `DROP INDEX CONCURRENTLY IF EXISTS idx_trace_usage_agent_materialized;`,
-- then restart the server so this migration builds it again.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_trace_usage_agent_materialized
    ON trace_usage (agent_id, materialized_at)
    WHERE agent_id IS NOT NULL;
