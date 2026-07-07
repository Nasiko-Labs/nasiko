-- =============================================================================
-- MCP Gateway schema
-- =============================================================================
--
-- Backs the `nasiko-mcp-gateway` crate. Mirrors the Python PoC's SQLite tables,
-- rebuilt for Postgres with platform conventions: UUID PKs (gen_random_uuid),
-- TIMESTAMPTZ, the shared set_updated_at() trigger, partial-unique indexes, and
-- FKs to the existing users(id) / agents(id) tables (ON DELETE CASCADE) so that
-- deleting a user or agent auto-removes their MCP rows — no manual orphan
-- cleanup (the PoC's jugaad).
--
-- Scope model:
--   * connections / credentials / oauth-tokens / composio-session  → per USER
--   * server-access + tool-permissions                             → per (USER, AGENT)
-- Backend/Composio sessions are per-user (O(users), not O(users×agents));
-- per-agent filtering happens at the aggregation layer via permissions_hash.
--
-- Enum-like columns use TEXT + CHECK (matches the PoC's string values and the
-- build_jobs.status pattern in 001_schema.sql) rather than ENUM types, so
-- adding an auth type later is a CHECK change, not an ALTER TYPE.

-- =============================================================================
-- mcp_auth_configs — Composio toolkit OAuth-app registrations
-- =============================================================================
-- Platform-level configs (is_platform = true, user_id NULL) are shared by all
-- users; user-scoped configs belong to one user. `auth_config_id` is the id
-- returned by Composio's auth_configs.create.
CREATE TABLE mcp_auth_configs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    auth_config_id TEXT NOT NULL UNIQUE,
    user_id UUID REFERENCES users(id) ON DELETE CASCADE,
    toolkit TEXT NOT NULL,
    auth_scheme TEXT NOT NULL DEFAULT 'OAUTH2',
    use_composio_managed BOOLEAN NOT NULL DEFAULT true,
    is_platform BOOLEAN NOT NULL DEFAULT false,
    display_name TEXT,
    logo_url TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Platform configs must not carry a user_id; user configs must.
    CONSTRAINT chk_mcp_auth_configs_scope CHECK (
        (is_platform AND user_id IS NULL) OR (NOT is_platform AND user_id IS NOT NULL)
    )
);
-- One platform config per toolkit; one user config per (user, toolkit).
CREATE UNIQUE INDEX uq_mcp_auth_configs_platform_toolkit
    ON mcp_auth_configs(toolkit) WHERE is_platform;
CREATE UNIQUE INDEX uq_mcp_auth_configs_user_toolkit
    ON mcp_auth_configs(user_id, toolkit) WHERE NOT is_platform;
CREATE INDEX idx_mcp_auth_configs_user ON mcp_auth_configs(user_id);
CREATE TRIGGER trg_mcp_auth_configs_updated_at
    BEFORE UPDATE ON mcp_auth_configs FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- =============================================================================
-- mcp_servers — generic (non-Composio) MCP server definitions
-- =============================================================================
-- Platform servers (is_platform = true, user_id NULL) are shared; user-scoped
-- servers are private. oauth_* columns are populated by auto-discovery + DCR
-- for auth_type = 'oauth2'.
CREATE TABLE mcp_servers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    url TEXT NOT NULL,
    transport TEXT NOT NULL DEFAULT 'streamable_http',
    auth_type TEXT NOT NULL DEFAULT 'none'
        CHECK (auth_type IN ('none', 'bearer', 'basic', 'oauth2', 'url_param')),
    -- For auth_type='url_param': the query-param name the server expects.
    url_param_name TEXT,
    -- For auth_type='bearer'/'basic': header to inject the credential into.
    -- NULL → Authorization.
    credential_header_name TEXT,
    -- Static headers (user-scoped servers only), JSON object.
    headers JSONB,
    description TEXT,
    display_name TEXT,
    logo_url TEXT,
    is_platform BOOLEAN NOT NULL DEFAULT false,
    user_id UUID REFERENCES users(id) ON DELETE CASCADE,
    is_active BOOLEAN NOT NULL DEFAULT true,
    -- OAuth 2.1 config (auto-discovery + dynamic client registration).
    oauth_authorization_endpoint TEXT,
    oauth_token_endpoint TEXT,
    oauth_client_id TEXT,
    oauth_client_secret TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT chk_mcp_servers_scope CHECK (
        (is_platform AND user_id IS NULL) OR (NOT is_platform AND user_id IS NOT NULL)
    )
);
-- One platform server per name; one user server per (name, user).
CREATE UNIQUE INDEX uq_mcp_servers_platform_name
    ON mcp_servers(name) WHERE is_platform;
CREATE UNIQUE INDEX uq_mcp_servers_user_name
    ON mcp_servers(name, user_id) WHERE NOT is_platform;
CREATE INDEX idx_mcp_servers_user ON mcp_servers(user_id);
CREATE INDEX idx_mcp_servers_active ON mcp_servers(is_active) WHERE is_active;
CREATE TRIGGER trg_mcp_servers_updated_at
    BEFORE UPDATE ON mcp_servers FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- =============================================================================
-- mcp_connections — per-user Composio OAuth connection state
-- =============================================================================
-- status: INITIATED → ACTIVE → EXPIRED. connected_account_id is the Composio
-- ca_… id, resolved on status sync and needed to link sessions to accounts.
CREATE TABLE mcp_connections (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    auth_config_id TEXT NOT NULL,
    toolkit TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'INITIATED'
        CHECK (status IN ('INITIATED', 'ACTIVE', 'EXPIRED')),
    connected_account_id TEXT,
    redirect_url TEXT,
    oauth_url TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- At most one non-EXPIRED connection per (user, toolkit) — enforces the PoC's
-- "one live connection" rule atomically instead of via IntegrityError catches.
CREATE UNIQUE INDEX uq_mcp_connections_user_toolkit_live
    ON mcp_connections(user_id, toolkit) WHERE status <> 'EXPIRED';
CREATE INDEX idx_mcp_connections_user ON mcp_connections(user_id);
CREATE INDEX idx_mcp_connections_account ON mcp_connections(connected_account_id)
    WHERE connected_account_id IS NOT NULL;
CREATE TRIGGER trg_mcp_connections_updated_at
    BEFORE UPDATE ON mcp_connections FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- =============================================================================
-- mcp_oauth_tokens — per-user OAuth 2.1 tokens for auth_type='oauth2' servers
-- =============================================================================
-- access_token / refresh_token are stored ENCRYPTED (SecretsCrypto::for_user).
CREATE TABLE mcp_oauth_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    mcp_server_id UUID NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    access_token TEXT NOT NULL,
    refresh_token TEXT,
    expires_at TIMESTAMPTZ,
    scope TEXT,
    token_type TEXT NOT NULL DEFAULT 'Bearer',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (mcp_server_id, user_id)
);
CREATE INDEX idx_mcp_oauth_tokens_user ON mcp_oauth_tokens(user_id);
CREATE TRIGGER trg_mcp_oauth_tokens_updated_at
    BEFORE UPDATE ON mcp_oauth_tokens FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- =============================================================================
-- mcp_user_credentials — per-user bearer/basic/url_param credentials
-- =============================================================================
-- credential_value is stored ENCRYPTED (SecretsCrypto::for_user) and is
-- write-only — never returned by any API.
CREATE TABLE mcp_user_credentials (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    mcp_server_id UUID NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    credential_type TEXT NOT NULL DEFAULT 'bearer'
        CHECK (credential_type IN ('bearer', 'basic', 'url_param')),
    credential_value TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (mcp_server_id, user_id)
);
CREATE INDEX idx_mcp_user_credentials_user ON mcp_user_credentials(user_id);
CREATE TRIGGER trg_mcp_user_credentials_updated_at
    BEFORE UPDATE ON mcp_user_credentials FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- =============================================================================
-- mcp_agent_server_access — per-(user, agent) server on/off toggle
-- =============================================================================
-- No row for a (user, agent, server_name) = server ENABLED by default
-- (opt-in restrictions only). server_type distinguishes composio toolkits from
-- generic MCP servers. agent_id CASCADEs so deleting an agent removes its rows.
CREATE TABLE mcp_agent_server_access (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    server_name TEXT NOT NULL,
    server_type TEXT NOT NULL CHECK (server_type IN ('composio', 'mcp')),
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, agent_id, server_name)
);
CREATE INDEX idx_mcp_agent_server_access_user_agent
    ON mcp_agent_server_access(user_id, agent_id);
CREATE INDEX idx_mcp_agent_server_access_server ON mcp_agent_server_access(server_name);
CREATE TRIGGER trg_mcp_agent_server_access_updated_at
    BEFORE UPDATE ON mcp_agent_server_access FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- =============================================================================
-- mcp_agent_tool_permissions — per-(user, agent, server) tool stances
-- =============================================================================
-- tool_pattern supports glob wildcards ('*', 'GMAIL_*', 'GMAIL_SEND_EMAIL').
-- stance priority: block > ask > allow. No rows for a server = all allowed.
CREATE TABLE mcp_agent_tool_permissions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    server_name TEXT NOT NULL,
    tool_pattern TEXT NOT NULL,
    stance TEXT NOT NULL CHECK (stance IN ('allow', 'ask', 'block')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, agent_id, server_name, tool_pattern)
);
CREATE INDEX idx_mcp_agent_tool_permissions_user_agent
    ON mcp_agent_tool_permissions(user_id, agent_id);
CREATE INDEX idx_mcp_agent_tool_permissions_server ON mcp_agent_tool_permissions(server_name);
CREATE TRIGGER trg_mcp_agent_tool_permissions_updated_at
    BEFORE UPDATE ON mcp_agent_tool_permissions FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- =============================================================================
-- mcp_composio_sessions — durable per-user Composio MCP session id
-- =============================================================================
-- One session_id per user, reused across requests. The resolved url + headers
-- are Redis-cached (not stored) since Composio may rotate them; the DB is the
-- source of truth only for the durable session_id.
CREATE TABLE mcp_composio_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    connected_account_ids JSONB,
    connected_toolkits JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TRIGGER trg_mcp_composio_sessions_updated_at
    BEFORE UPDATE ON mcp_composio_sessions FOR EACH ROW EXECUTE FUNCTION set_updated_at();
