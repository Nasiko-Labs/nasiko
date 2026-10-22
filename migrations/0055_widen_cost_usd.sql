-- Widen the per-row cost columns so one row can record $100 or more.
--
-- NUMERIC(10, 8) leaves two integer digits, so any cost of $100 or more fails
-- with "numeric field overflow". A long coding-agent turn crosses that: the
-- ingest route returned HTTP 500, and the CLI quarantined the event after its
-- retries, so the turn's usage was never recorded.
--
-- NUMERIC(18, 8) allows up to $9,999,999,999.99999999 per row. The scale is
-- unchanged, so stored values and their `rust_decimal::Decimal` reads stay
-- exactly as they are. Raising only a NUMERIC's precision does not rewrite the
-- tables; `idx_token_usage_cost` is rebuilt because it indexes `cost_usd`.
--
-- Postgres refuses to alter a column that a view selects, and
-- `token_usage_daily` selects `token_usage.cost_usd`. The view is dropped and
-- recreated exactly as in 0001_schema.sql, with its unique index.

DROP MATERIALIZED VIEW token_usage_daily;

ALTER TABLE token_usage ALTER COLUMN cost_usd TYPE NUMERIC(18, 8);
ALTER TABLE chat_messages ALTER COLUMN cost_usd TYPE NUMERIC(18, 8);
ALTER TABLE flows ALTER COLUMN total_cost_usd TYPE NUMERIC(18, 8);

CREATE MATERIALIZED VIEW token_usage_daily AS
SELECT user_id, agent_id, operation_type, provider, model,
    DATE(created_at) as date, COUNT(*) as request_count,
    SUM(input_tokens) as total_input_tokens, SUM(output_tokens) as total_output_tokens,
    SUM(total_tokens) as total_tokens, SUM(cache_creation_input_tokens) as total_cache_creation_tokens,
    SUM(cache_read_input_tokens) as total_cache_read_tokens, SUM(cached_tokens) as total_cached_tokens,
    SUM(reasoning_tokens) as total_reasoning_tokens, SUM(cost_usd) as total_cost_usd,
    AVG(latency_ms) as avg_latency_ms
FROM token_usage
GROUP BY user_id, agent_id, operation_type, provider, model, DATE(created_at);

CREATE UNIQUE INDEX idx_token_usage_daily_unique
    ON token_usage_daily(user_id, COALESCE(agent_id, '00000000-0000-0000-0000-000000000000'::uuid), operation_type, provider, model, date);
