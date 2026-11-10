-- no-transaction
-- The idle reaper scans live sessions (running, starting) by how long they have been idle, and
-- recovers stuck suspend claims (stopping) by the deadline kept in lease_expires_at. A partial
-- index over the live states keeps both scans cheap when most sessions are parked.
-- CONCURRENTLY for the same reason as 0071: `chat_sessions` is an existing table, and a plain
-- CREATE INDEX would block its writes while it builds. One statement per file, because a
-- no-transaction migration runs outside any transaction and a failed one must be re-runnable.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_chat_sessions_reapable
    ON chat_sessions (last_turn_at, lease_expires_at)
    WHERE runtime_state IN ('running', 'starting', 'stopping');
