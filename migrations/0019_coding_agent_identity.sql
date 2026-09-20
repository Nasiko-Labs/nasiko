ALTER TABLE agents
    ADD COLUMN coding_agent_integration_id TEXT
    CHECK (coding_agent_integration_id IN ('claude', 'opencode', 'codex', 'cursor'));

CREATE UNIQUE INDEX agents_owner_coding_integration_active_uniq
    ON agents (owner_id, coding_agent_integration_id)
    WHERE coding_agent_integration_id IS NOT NULL AND deleted_at IS NULL;
