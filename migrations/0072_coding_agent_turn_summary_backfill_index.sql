-- no-transaction
-- Finds receipts the turn-summary backfill (0068) has yet to reach, in bounded chunks. Empty once
-- the backfill is done, so it costs nothing afterwards. CONCURRENTLY for the same reason as 0071.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_coding_agent_telemetry_turn_summary_pending
    ON coding_agent_telemetry_events (user_id, event_id)
    WHERE turn_started_at IS NULL;
