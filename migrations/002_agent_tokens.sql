-- Allow agent tokens to be recorded and revoked.
--
-- Previously auth_tokens.user_id was NOT NULL + FK to users.
-- Agent tokens have an agent UUID as subject, which is not in the users table,
-- so they could never be recorded → could never be revoked.
--
-- This migration makes user_id nullable and adds an agent_id column so that
-- each token belongs to exactly one subject: a user OR an agent.

ALTER TABLE auth_tokens ALTER COLUMN user_id DROP NOT NULL;
ALTER TABLE auth_tokens ADD COLUMN agent_id UUID REFERENCES agents(id) ON DELETE CASCADE;

-- Exactly one of user_id / agent_id must be set.
ALTER TABLE auth_tokens ADD CONSTRAINT auth_tokens_one_subject CHECK (
    (user_id IS NOT NULL)::int + (agent_id IS NOT NULL)::int = 1
);

-- Index for fast per-agent revocation lookups (mirrors the existing user index).
CREATE INDEX idx_auth_tokens_agent_active
    ON auth_tokens(agent_id, expires_at)
    WHERE revoked_at IS NULL AND agent_id IS NOT NULL;
