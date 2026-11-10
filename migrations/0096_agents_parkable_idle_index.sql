-- no-transaction
-- The idle reaper parks a running framework agent whose parking policy is `auto` once nothing has
-- happened to it for `AGENT_PARK_IDLE_SECS`: no request (`last_request_at`) and no change to its
-- row (`updated_at`, which a deploy moves), whichever is later. It asks once per tick, so this
-- partial index covers exactly the rows that can be parked, keyed by that instant: the scan reads
-- the idle ones and never the rest of a large fleet. The query must spell the expression and the
-- predicate the same way to use it.
-- CONCURRENTLY for the same reason as 0090: `agents` is an existing table, and a plain CREATE INDEX
-- would block its writes while it builds. One statement per file, because a no-transaction
-- migration runs outside any transaction.
-- If the build fails, Postgres keeps an INVALID index under this name and IF NOT EXISTS would skip
-- it on the next start: run `DROP INDEX CONCURRENTLY IF EXISTS idx_agents_parkable_idle;`, then
-- restart the server so this migration builds it again.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_agents_parkable_idle
    ON agents ((GREATEST(last_request_at, updated_at)))
    WHERE status = 'running'
      AND workload_kind = 'agent'
      AND parking_policy = 'auto'
      AND deleted_at IS NULL
      AND image IS NOT NULL;
