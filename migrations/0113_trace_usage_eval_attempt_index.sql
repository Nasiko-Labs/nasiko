-- no-transaction
-- `GET /api/evals/usage?run=<run>` totals one run's attempts: the caller's `trace_usage` rows whose
-- `eval_attempt` starts with `<run>:` (0112). Only eval rows carry the column, so a partial index
-- over them stays as small as the evals themselves. `text_pattern_ops` compares byte by byte, which
-- is what a prefix range needs whatever the database collation: the read asks for
-- `eval_attempt ~>=~ '<run>:' AND eval_attempt ~<~ '<run>;'`.
-- CONCURRENTLY so building it never blocks the router's live upserts. One statement per file,
-- because a no-transaction migration runs outside any transaction.
-- If the build fails, Postgres keeps an INVALID index under this name and IF NOT EXISTS would skip
-- it on the next start: run `DROP INDEX CONCURRENTLY IF EXISTS idx_trace_usage_eval_attempt;`,
-- then restart the server so this migration builds it again.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_trace_usage_eval_attempt
    ON trace_usage (eval_attempt text_pattern_ops)
    WHERE eval_attempt IS NOT NULL;
