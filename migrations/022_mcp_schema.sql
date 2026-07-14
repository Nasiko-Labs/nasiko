-- =============================================================================
-- MCP Gateway schema — unified connectors + owner-controlled sharing
-- =============================================================================
--
-- Backs the `nasiko-mcp-gateway` crate. A single `mcp_connectors` registry holds
-- both Composio toolkits and custom MCP servers (discriminated by
-- `provider_type`), keyed by connector id (UUID), with owner-controlled sharing
-- (`mcp_connector_grants`, mirroring the platform's existing `agent_grants`
-- shape). Enum-like columns use TEXT + CHECK (matches the platform's
-- build_jobs.status / agent-grants pattern) so adding a provider or auth type
-- later is a CHECK change, not an ALTER TYPE.
--
-- Design invariants baked in (enforced in crate code; see
-- MCP_GATEWAY_TECHNICAL_DESIGN.md §18):
--   #1 tool-routing prefix derives from id, never name (names collide across
--      owners once sharing exists).
--   #2 revoking a grant deletes the grantee's connection row.
--   #3 provider CHECK requires the correct provider's fields, not just forbids.
--   #4 no credential_type column — derived by joining connectors.auth_type.
--   #5 owner_id is ON DELETE RESTRICT — deleting a user can't destroy a shared
--      connector out from under everyone it was shared with.

-- =============================================================================
-- mcp_connectors — the unified registry
-- =============================================================================
-- One row per Composio toolkit OR per custom MCP server. `provider_type` says
-- which; provider-specific columns are populated only for the matching type.
--   * composio   : owner_id IS NULL (globally connectable; each user connects
--                  their own account). auth_config_id set; url NULL.
--   * mcp_server : owner_id always set (even admin-created) — private until the
--                  owner shares it. url set; auth_config_id NULL.
-- `name` is a display/search label only, NEVER the tool-routing key (that is
-- derived from `id` in crate code — invariant #1).
CREATE TABLE mcp_connectors (
    id                            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider_type                 TEXT NOT NULL CHECK (provider_type IN ('composio', 'mcp_server')),
    owner_id                      UUID REFERENCES users(id) ON DELETE RESTRICT,
    name                          TEXT NOT NULL,
    display_name                  TEXT,
    logo_url                      TEXT,
    description                   TEXT,

    -- Composio-only (NULL when provider_type = 'mcp_server')
    auth_config_id                TEXT,
    auth_scheme                   TEXT DEFAULT 'OAUTH2',
    use_composio_managed          BOOLEAN,

    -- MCP-server-only (NULL when provider_type = 'composio')
    url                           TEXT,
    transport                     TEXT DEFAULT 'streamable_http',
    auth_type                     TEXT CHECK (auth_type IN ('none', 'bearer', 'basic', 'oauth2', 'url_param')),
    url_param_name                TEXT,
    credential_header_name        TEXT DEFAULT 'Authorization',
    headers                       JSONB,
    is_active                     BOOLEAN DEFAULT true,
    oauth_authorization_endpoint  TEXT,
    oauth_token_endpoint          TEXT,
    oauth_client_id               TEXT,
    oauth_client_secret           TEXT,   -- encrypted at rest (SecretsCrypto)

    created_at                    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                    TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Two-directional: requires the correct provider's fields, not just forbids
    -- the wrong provider's (invariant #3) — an empty, unusable connector cannot
    -- be inserted.
    CONSTRAINT chk_connectors_provider_fields CHECK (
        (provider_type = 'composio'   AND auth_config_id IS NOT NULL AND url IS NULL) OR
        (provider_type = 'mcp_server' AND url IS NOT NULL AND auth_config_id IS NULL)
    )
);
-- name is unique within one owner's scope; platform (composio, owner_id NULL)
-- names are globally unique. Two different owners MAY share a name — which is
-- exactly why the tool-routing prefix comes from id, not name (invariant #1).
CREATE UNIQUE INDEX uq_mcp_connectors_name_owner
    ON mcp_connectors(name, owner_id) WHERE owner_id IS NOT NULL;
CREATE UNIQUE INDEX uq_mcp_connectors_name_platform
    ON mcp_connectors(name) WHERE owner_id IS NULL;
CREATE INDEX idx_mcp_connectors_owner ON mcp_connectors(owner_id);
CREATE INDEX idx_mcp_connectors_provider ON mcp_connectors(provider_type);
CREATE INDEX idx_mcp_connectors_active ON mcp_connectors(is_active) WHERE is_active;
CREATE TRIGGER trg_mcp_connectors_updated_at
    BEFORE UPDATE ON mcp_connectors FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- =============================================================================
-- mcp_connector_grants — owner-controlled sharing
-- =============================================================================
-- Mirrors agent_grants (0001_schema.sql): grant_type 'user' targets a specific
-- user id, 'public' targets everyone via the '*' sentinel. Only meaningful for
-- provider_type='mcp_server' connectors (composio is always globally connectable
-- and has no owner). Revoking a grant must also delete the grantee's
-- mcp_user_connections row for the connector — enforced transactionally in crate
-- code (invariant #2), not by the schema.
CREATE TABLE mcp_connector_grants (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    connector_id  UUID NOT NULL REFERENCES mcp_connectors(id) ON DELETE CASCADE,
    grant_type    TEXT NOT NULL CHECK (grant_type IN ('user', 'public')),
    grantee_id    TEXT NOT NULL,              -- a user id as text, or '*' for everyone
    granted_by    UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (connector_id, grant_type, grantee_id),
    CONSTRAINT chk_mcp_grants_public_sentinel CHECK (
        (grant_type = 'public' AND grantee_id = '*') OR
        (grant_type <> 'public' AND grantee_id <> '*')
    )
);
CREATE INDEX idx_mcp_grants_grantee ON mcp_connector_grants(grantee_id, grant_type);
CREATE INDEX idx_mcp_grants_connector ON mcp_connector_grants(connector_id);

-- =============================================================================
-- mcp_connector_tools — synced tool catalog
-- =============================================================================
-- Persisted so permission-configuration screens render instantly without a live
-- backend call. A cache of what the live backend reports (not the enforcement
-- source of truth); last_synced_at makes staleness visible rather than silent.
CREATE TABLE mcp_connector_tools (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    connector_id    UUID NOT NULL REFERENCES mcp_connectors(id) ON DELETE CASCADE,
    tool_name       TEXT NOT NULL,
    description     TEXT,
    default_stance  TEXT NOT NULL DEFAULT 'allow' CHECK (default_stance IN ('allow', 'ask', 'block')),
    last_synced_at  TIMESTAMPTZ,

    UNIQUE (connector_id, tool_name)
);
CREATE INDEX idx_mcp_connector_tools_connector ON mcp_connector_tools(connector_id);

-- =============================================================================
-- mcp_user_connections — per-user credential / connection state
-- =============================================================================
-- One row per (user, connector), covering every auth shape uniformly. Never
-- shared, even when the connector itself is. No credential_type column — the
-- format is read by joining mcp_connectors.auth_type (invariant #4). All
-- *encrypted_* columns are stored ENCRYPTED (SecretsCrypto::for_user) and are
-- write-only — never returned by any API.
CREATE TABLE mcp_user_connections (
    id                        UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id                   UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    connector_id              UUID NOT NULL REFERENCES mcp_connectors(id) ON DELETE CASCADE,
    status                    TEXT NOT NULL DEFAULT 'INITIATED'
                                  CHECK (status IN ('INITIATED', 'ACTIVE', 'EXPIRED')),

    -- Composio-flavored fields (NULL for custom MCP servers)
    connected_account_id      TEXT,
    redirect_url              TEXT,
    oauth_url                 TEXT,

    -- Credential storage — format determined by joining mcp_connectors.auth_type.
    encrypted_credential      TEXT,   -- bearer / basic / url_param value OR OAuth access token
    encrypted_refresh_token   TEXT,   -- oauth2 only
    token_expires_at          TIMESTAMPTZ,  -- oauth2 only
    scope                     TEXT,         -- oauth2 only

    created_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (user_id, connector_id)
);
CREATE INDEX idx_mcp_user_connections_user ON mcp_user_connections(user_id);
CREATE INDEX idx_mcp_user_connections_connector ON mcp_user_connections(connector_id);
CREATE INDEX idx_mcp_user_connections_account ON mcp_user_connections(connected_account_id)
    WHERE connected_account_id IS NOT NULL;
CREATE TRIGGER trg_mcp_user_connections_updated_at
    BEFORE UPDATE ON mcp_user_connections FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- =============================================================================
-- mcp_composio_sessions — Composio's durable Tool Router session id
-- =============================================================================
-- One session id per user, reused across requests. Composio's Tool Router
-- session concept has no custom-server equivalent, so it stays its own table.
-- Nothing about which toolkits are connected is cached here — that is derived
-- live from mcp_user_connections, so the two can never drift out of sync.
CREATE TABLE mcp_composio_sessions (
    id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id               UUID NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    composio_session_id   TEXT NOT NULL,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TRIGGER trg_mcp_composio_sessions_updated_at
    BEFORE UPDATE ON mcp_composio_sessions FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- =============================================================================
-- mcp_agent_connector_access — per (user, agent, connector) permission override
-- =============================================================================
-- user_id is the CALLER (an agent can be invoked by its owner or by anyone
-- granted access via agent_grants — each caller's restrictions apply
-- independently). The single most important rule: NO ROW = fully allowed. A row
-- only ever exists to restrict — disable the whole connector for the agent, or
-- apply per-tool allow/ask/block rules. tool_rules is a JSONB array of
-- {pattern, stance}; app code validates/dedupes it on write (the JSONB array has
-- no DB-level uniqueness — accepted tradeoff). This table must never be consulted
-- on its own: every access check confirms connector reachability (owner/grant)
-- FIRST (§12), so a stale enabled=true row can never re-admit a revoked grant.
CREATE TABLE mcp_agent_connector_access (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id        UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    agent_id       UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    connector_id   UUID NOT NULL REFERENCES mcp_connectors(id) ON DELETE CASCADE,
    enabled        BOOLEAN NOT NULL DEFAULT true,
    tool_rules     JSONB NOT NULL DEFAULT '[]',   -- [{ "pattern": "SEND_*", "stance": "block" }, ...]
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (user_id, agent_id, connector_id)
);
CREATE INDEX idx_mcp_agent_connector_access_user_agent
    ON mcp_agent_connector_access(user_id, agent_id);
CREATE INDEX idx_mcp_agent_connector_access_connector
    ON mcp_agent_connector_access(connector_id);
CREATE TRIGGER trg_mcp_agent_connector_access_updated_at
    BEFORE UPDATE ON mcp_agent_connector_access FOR EACH ROW EXECUTE FUNCTION set_updated_at();
