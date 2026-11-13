-- MCP connector upload status (enterprise-free, OSS: anyone can upload their own MCP server).
--
-- Mirrors `upload_status` (0001_schema.sql) for agent uploads: the connector a first-time
-- upload creates is deleted entirely on build failure (`delete_mcp_connector_or_mark_failed`,
-- mirroring `delete_agent_or_mark_failed`), which is by design — a connector that never once
-- worked has no history worth keeping, and keeping it blocks re-uploading the same name. But
-- that means the failure reason dies with it unless something survives the connector's own
-- lifecycle. This table is that something: keyed by `upload_id` (= the build's id, same trick
-- `upload_status.upload_id` uses), so the CLI/UI can poll it long after the connector row — and
-- its build record — are both gone.
CREATE TABLE mcp_upload_status (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    upload_id      TEXT NOT NULL UNIQUE,
    connector_id   UUID REFERENCES mcp_connectors(id) ON DELETE SET NULL,
    connector_name TEXT NOT NULL,
    -- Same vocabulary as mcp_connector_builds.status (0003_mcp.sql) — this table tracks the
    -- same lifecycle, just independently of the connector/build rows' own survival.
    status         TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'building', 'success', 'failed')),
    owner_id       UUID REFERENCES users(id) ON DELETE SET NULL,
    error_message  TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_mcp_upload_status_owner ON mcp_upload_status(owner_id, created_at DESC);
CREATE INDEX idx_mcp_upload_status_connector ON mcp_upload_status(connector_id);
CREATE TRIGGER trg_mcp_upload_status_updated_at
    BEFORE UPDATE ON mcp_upload_status FOR EACH ROW EXECUTE FUNCTION set_updated_at();
