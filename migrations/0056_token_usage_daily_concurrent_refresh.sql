-- Let `token_usage_daily` refresh concurrently.
--
-- The hourly task in server/src/state.rs runs REFRESH MATERIALIZED VIEW
-- CONCURRENTLY, which needs a unique index on plain columns with no WHERE
-- clause. The index from 0001_schema.sql wraps `agent_id` in COALESCE so that
-- NULL agents collide, which makes it an expression index, so every refresh
-- failed and the view stayed as it was when first created.
--
-- NULLS NOT DISTINCT (Postgres 15+; the stack runs 16) gives the same
-- uniqueness on the plain columns: NULLs compare equal, just as GROUP BY in the
-- view already groups them. Only the index changes; the view, its columns and
-- its rows are untouched.

DROP INDEX idx_token_usage_daily_unique;

CREATE UNIQUE INDEX idx_token_usage_daily_unique
    ON token_usage_daily (user_id, agent_id, operation_type, provider, model, date)
    NULLS NOT DISTINCT;
