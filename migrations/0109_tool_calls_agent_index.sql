-- no-transaction
-- The Overview's estate counts each visible agent's gateway tool calls over the last 14 days:
-- `tool_kind = 'mcp' AND agent_id = ANY(...) AND started_at >= ...`. `tool_calls` had no index by
-- agent, so every estate read scanned the whole platform's tool calls in the window
-- (`idx_tool_calls_started`), and under load it was the slowest statement on the page. Keyed by
-- agent, then time, the read takes only the visible agents' rows; the per-caller half of the
-- estate's rule keeps using `idx_tool_calls_user_time`.
-- CONCURRENTLY so building it never blocks the gateway's live inserts. One statement per file,
-- because a no-transaction migration runs outside any transaction.
-- If the build fails, Postgres keeps an INVALID index under this name and IF NOT EXISTS would skip
-- it on the next start: run `DROP INDEX CONCURRENTLY IF EXISTS idx_tool_calls_agent_started;`,
-- then restart the server so this migration builds it again.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_tool_calls_agent_started
    ON tool_calls (agent_id, started_at)
    WHERE tool_kind = 'mcp' AND agent_id IS NOT NULL;
