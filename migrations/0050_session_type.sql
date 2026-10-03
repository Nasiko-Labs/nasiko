-- =============================================================================
-- Session type
--
-- `chat_sessions` holds three kinds of session that were only ever told apart
-- by guesswork on other columns: an orchestrator-routed chat (agent_id NULL),
-- a direct agent chat (agent_id set), and a MAF execution (also agent_id NULL,
-- so indistinguishable from the orchestrator one — which is why MAF runs leak
-- into the Orchestrator session list). `session_type` records it explicitly.
--
-- Default 'direct_chat': every writer that binds a concrete agent_id
-- (agent_proxy, coding-agent telemetry, HITL) is a direct chat, so only the
-- two other writers have to say anything.
-- =============================================================================

ALTER TABLE chat_sessions
    ADD COLUMN session_type TEXT NOT NULL DEFAULT 'direct_chat'
        CHECK (session_type IN ('orchestrator', 'direct_chat', 'maf_execution'));

-- Backfill. Orchestrator sessions are the ones dispatch/HITL stamped with the
-- orchestrator proxy path, plus older rows that bound no agent at all.
UPDATE chat_sessions
   SET session_type = 'orchestrator'
 WHERE agent_url = '/api/orchestrator/a2a'
    OR (agent_id IS NULL AND agent_url IS NULL);

-- MAF runs use the execution UUID as the session id, so they are recoverable
-- exactly; run last so it wins over the orchestrator pass above.
UPDATE chat_sessions cs
   SET session_type = 'maf_execution'
  FROM maf_executions e
 WHERE cs.session_id = e.id::text;
