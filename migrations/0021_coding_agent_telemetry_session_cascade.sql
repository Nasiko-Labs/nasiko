-- Deleting a chat session takes its coding-agent telemetry receipts with it.
--
-- 0017 declared this FK ON DELETE RESTRICT, which made the hard DELETE in
-- chat/routes.rs::delete_session fail for any coding-agent session that had
-- ingested a turn — and fail *after* the handler had already dropped the
-- session's chat_message_files rows and their S3 blobs, so the caller lost
-- their attachments, got a 500, and kept an undeletable session.
--
-- Receipts are an OTLP delivery outbox plus a dedup ledger, not a system of
-- record: the only readers are the exporter and the ingest replay check, and
-- FinOps reads Tempo/Loki rather than this table. The same table already
-- cascades on user delete, and sibling session-scoped data (chat_messages)
-- cascades too — so session delete cascades here as well.
ALTER TABLE coding_agent_telemetry_events
    DROP CONSTRAINT coding_agent_telemetry_events_session_id_fkey;

ALTER TABLE coding_agent_telemetry_events
    ADD CONSTRAINT coding_agent_telemetry_events_session_id_fkey
    FOREIGN KEY (session_id) REFERENCES chat_sessions(session_id) ON DELETE CASCADE;
