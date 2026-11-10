-- no-transaction
-- Index for sandbox metering. CONCURRENTLY for the same reason as 0071.
--
-- The meter resolves every Running session sandbox by `chat_sessions.sandbox_ref`, once a tick.
-- Partial: sessions without a sandbox cost nothing.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_chat_sessions_sandbox_ref
    ON chat_sessions (sandbox_ref)
    WHERE sandbox_ref IS NOT NULL;
