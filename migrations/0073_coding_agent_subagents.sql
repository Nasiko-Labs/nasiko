-- Subagent sessions: a coding-agent session another session of the same agent spawned (a Claude
-- Code Task, an OpenCode child session, a Cursor subagent chat, a Codex spawned thread).
--
-- Set by ingest from the event's `session.parent`, first non-null value wins per column. No foreign
-- key: a child can arrive before its parent (an import walks transcripts in any order), and the
-- link resolves as soon as the parent's row exists. `parent_session_id` is the parent's scoped id,
-- derived exactly as the child's own `session_id` is.
--
-- Nullable columns without defaults: catalog-only, no row is rewritten.
ALTER TABLE chat_sessions
    ADD COLUMN parent_session_id TEXT NULL,
    ADD COLUMN spawn_tool_call_id TEXT NULL,
    ADD COLUMN subagent_type TEXT NULL;
