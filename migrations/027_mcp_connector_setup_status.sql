-- Connector-level setup status for the plain URL-connect/probe flow (NOT the
-- separate upload/GitHub-import flow, which owns its own `build_status`
-- column under a different CHECK constraint — this must never touch that).
--
-- `mcp_user_connections.status` already tracks whether ONE user has
-- personally connected; this tracks whether the connector itself is usable
-- yet at all — relevant for auth_type='oauth2'/credential-requiring servers,
-- where registering the connector and finishing setup (a credential, or a
-- browser OAuth round-trip) are two separate steps.
ALTER TABLE mcp_connectors
    ADD COLUMN setup_status TEXT CHECK (setup_status IN ('pending', 'active', 'failed')),
    ADD COLUMN setup_error TEXT;

-- Existing rows are all fully usable already (registered before this concept
-- existed) — mcp_server rows created via the old flow are unambiguously active;
-- composio rows don't use this field at all (left NULL).
UPDATE mcp_connectors SET setup_status = 'active' WHERE provider_type = 'mcp_server';
