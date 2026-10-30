-- Per-connector OAuth overrides. Identity providers behind proxies (Entra ID
-- behind Azure API Management) cannot be classified from metadata alone, so
-- the owner can pin what the authorize request asks for.
--   oauth_scopes: explicit space-separated scope; when set it replaces
--     discovery's scope selection.
--   oauth_send_resource: NULL lets discovery decide; true always sends the
--     connector URL as the RFC 8707 `resource`; false never sends it.
ALTER TABLE mcp_connectors
    ADD COLUMN oauth_scopes TEXT,
    ADD COLUMN oauth_send_resource BOOLEAN;
