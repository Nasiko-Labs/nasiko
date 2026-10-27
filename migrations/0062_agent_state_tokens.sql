-- agent_state_tokens - one live agent state API credential per agent.
--
-- The MCP gateway credential (`agent_gateway_tokens`, MCP_GATEWAY_TOKEN) also
-- travels inside URL paths (`/api/mcp/s/{token}`, for MCP clients that accept
-- only a URL), where proxies and access logs can record it, so it must not also
-- open the agent's durable state. The state API (/api/agent-state) therefore has
-- its own credential, injected as NASIKO_AGENT_TOKEN.
--
-- A separate table, not a scope column on `agent_gateway_tokens`: servers from
-- before this migration keep running during a rolling upgrade. They mint with
-- `ON CONFLICT (agent_id)` and authenticate the gateway with no notion of scope,
-- so widening that table's key would break their mint and let them accept a
-- state credential at /api/mcp. Leaving it untouched keeps both doors apart for
-- old and new servers alike.
--
-- Same shape and lifecycle as `agent_gateway_tokens` (0008, 0009): only the
-- SHA-256 hex hash is stored, a mint rotates it (upsert) with the superseded hash
-- kept for a bounded grace, and destroy tombstones it via revoked_at.
CREATE TABLE agent_state_tokens (
    agent_id UUID PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL,
    prev_token_hash TEXT,
    rotated_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ
);

-- Bearer-token authentication looks up by either hash; the grace window is
-- applied in the query, not the index.
CREATE INDEX idx_agent_state_tokens_hash ON agent_state_tokens(token_hash)
    WHERE revoked_at IS NULL;
CREATE INDEX idx_agent_state_tokens_prev_hash
    ON agent_state_tokens(prev_token_hash)
    WHERE prev_token_hash IS NOT NULL AND revoked_at IS NULL;
