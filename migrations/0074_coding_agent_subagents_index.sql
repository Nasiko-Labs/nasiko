-- no-transaction
-- A parent's children, for `session/list` (subagent totals per page row, `parent=`) and the
-- session detail. Partial: only subagent sessions are indexed, so every other session costs
-- nothing. CONCURRENTLY for the same reason as 0071.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_chat_sessions_parent_session_id
    ON chat_sessions (parent_session_id)
    WHERE parent_session_id IS NOT NULL;
