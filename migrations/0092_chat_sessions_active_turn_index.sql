-- no-transaction
-- The lookup `tasks/*` make for the live turn (`active_turn_id`). See 0086 for why this is its
-- own file.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_chat_sessions_active_turn
    ON chat_sessions (active_turn_id) WHERE active_turn_id IS NOT NULL;
