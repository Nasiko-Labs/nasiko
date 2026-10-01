-- Revocation list for ACP coding-agent session tokens (NAS-839).
--
-- A session token is a short-lived, signed credential carrying
-- {agent_id, user_id, session_id, flow_id, jti}. Verification is offline (the
-- signature is the proof), so to revoke a token before its `exp` the control
-- plane records its `jti` here; the MCP gateway and LLM router reject any token
-- whose `jti` is present.
--
-- Rows are safe to purge once `expires_at` has passed (the token can no longer
-- verify regardless), so the table stays small.
CREATE TABLE IF NOT EXISTS revoked_session_tokens (
    jti        TEXT PRIMARY KEY,
    revoked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- The token's own `exp`, so a sweeper can drop entries that can no longer verify.
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_revoked_session_tokens_expires_at
    ON revoked_session_tokens (expires_at);
