-- A provider_type='system' connector with auto_enable=true is enabled for every
-- agent by default (unioned into each agent's enabled set at the gateway's sole
-- PermissionContext builder, load_permission_context), independent of any
-- mcp_agent_connector_access row. Additive to 0054_workspace.sql's system kind.
ALTER TABLE mcp_connectors ADD COLUMN auto_enable BOOLEAN NOT NULL DEFAULT false;
