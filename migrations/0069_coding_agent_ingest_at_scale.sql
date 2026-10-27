-- Coding-agent history imports at scale: background work ordering and turns Tempo cannot hold.
--
-- Every statement here is a catalog change only — no row is read or rewritten — so the deploy
-- holds its table lock for milliseconds however many receipts exist.

-- A turn that ended more than an hour before the server received it is history (an import or a
-- laptop catching up), not a live session. Background work (OTLP export, Tempo read-back) takes
-- live turns first, so an import never delays someone's live session. Set by ingest; a constant
-- default is a metadata-only addition, and treating pre-existing receipts as live is harmless.
ALTER TABLE coding_agent_telemetry_events
    ADD COLUMN otlp_historical BOOLEAN NOT NULL DEFAULT false;

-- 'skipped': the turn is older than Tempo's ingestion window
-- (`CODING_AGENT_TEMPO_MAX_AGE_SECS`, matching `ingestion_time_range_slack` in tempo.yaml), so it
-- is never exported — Tempo would drop the spans — and never read back. Its spend, written to
-- `trace_usage` at ingest, comes from Postgres alone. A state rather than fake delivery
-- timestamps: the delivered columns keep meaning "the collector accepted it".
--
-- NOT VALID so adding the constraint does not scan the table under this migration's exclusive
-- lock; 0070 validates it under a lock that does not block reads or writes. The new set is a
-- superset of the old one, so validation cannot fail.
ALTER TABLE coding_agent_telemetry_events
    DROP CONSTRAINT coding_agent_telemetry_events_otlp_state_check,
    ADD CONSTRAINT coding_agent_telemetry_events_otlp_state_check
        CHECK (otlp_state IN ('pending', 'processing', 'delivered', 'failed', 'skipped')) NOT VALID;
