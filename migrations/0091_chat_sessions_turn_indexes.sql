-- no-transaction
-- The lookup `tasks/get` makes for a finished turn (`last_turn_id`). Partial, so sessions without
-- one cost nothing. CONCURRENTLY for the same reason as 0071, and one statement per file: several
-- statements sent together run in an implicit transaction, where CONCURRENTLY is refused.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_chat_sessions_last_turn
    ON chat_sessions (last_turn_id) WHERE last_turn_id IS NOT NULL;
