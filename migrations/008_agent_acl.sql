-- =============================================================================
-- Agent-to-Agent ACL
-- Replaces cross_team_agent_access (unused, premature).
-- Defines which agents a caller agent is permitted to invoke.
-- Allowlist semantics: no rows = unrestricted, any rows = only listed targets.
-- =============================================================================

DROP TABLE IF EXISTS cross_team_agent_access;

CREATE TABLE agent_acl (
    caller_agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    target_agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    granted_by UUID REFERENCES users(id),
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (caller_agent_id, target_agent_id)
);

CREATE INDEX idx_agent_acl_caller ON agent_acl(caller_agent_id);
