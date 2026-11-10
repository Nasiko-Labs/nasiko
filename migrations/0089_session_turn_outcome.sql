-- What A2A `tasks/*` needs to know about a harness session's turns.
--
-- The A2A task id of a turn is its turn id. `active_turn_id` already names the live one; these
-- record the most recent finished one and its outcome (`completed`, `failed` or `canceled`), so
-- `tasks/get` can answer after the turn ends, and a cancel request that another replica must
-- observe. Only the latest turn is remembered: an older task id is a `TaskNotFound`.
ALTER TABLE chat_sessions
    ADD COLUMN IF NOT EXISTS last_turn_id TEXT,
    ADD COLUMN IF NOT EXISTS last_turn_state TEXT
        CHECK (last_turn_state IN ('completed', 'failed', 'canceled')),
    ADD COLUMN IF NOT EXISTS cancel_requested_at TIMESTAMPTZ;
-- (Its two lookup indexes are built concurrently in 0086 and 0087.)
