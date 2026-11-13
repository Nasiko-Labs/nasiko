-- The MCP Builds page (mirrors the Agents Builds page) wants a "Source" column, the same as
-- `mcp_connector_builds` already distinguishes via `github_url IS NOT NULL` vs a zip. Recorded once,
-- fixed for the life of the upload, since an upload's source never changes after it's queued.
ALTER TABLE mcp_upload_status
    ADD COLUMN source TEXT NOT NULL DEFAULT 'zip' CHECK (source IN ('zip', 'github'));
ALTER TABLE mcp_upload_status ALTER COLUMN source DROP DEFAULT;
