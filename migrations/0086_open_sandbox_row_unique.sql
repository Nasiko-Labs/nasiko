-- no-transaction
-- Its own file: CONCURRENTLY cannot share a (simple-query) transaction with another statement.
-- At most one open metering row per sandbox, so two control-plane replicas that both see a
-- sandbox with no backend start time cannot each open (and double-bill) a row.
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_agent_instance_sessions_open_sandbox
    ON agent_instance_sessions (instance_key)
    WHERE kind = 'sandbox' AND ended_at IS NULL;
