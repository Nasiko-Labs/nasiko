-- no-transaction
-- The OTLP outbox claim walks the distinct users with exportable receipts, then takes each
-- user's live turns before their historical ones (`coding_agent_otlp::claim_batch`). This serves
-- both steps and holds only receipts still in the outbox, so it stays small once exports drain.
--
-- CONCURRENTLY, so building it never blocks ingest; that cannot run in a transaction, hence the
-- directive above and one statement in this file.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_coding_agent_telemetry_otlp_claim
    ON coding_agent_telemetry_events (user_id, otlp_historical, received_at)
    WHERE otlp_state IN ('pending', 'failed', 'processing');
