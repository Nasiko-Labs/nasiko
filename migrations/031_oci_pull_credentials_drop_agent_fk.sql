-- Drop the agents(id) foreign key on oci_pull_credentials so the column can
-- hold MCP connector UUIDs too (uploaded MCP servers need pull credentials
-- the same way agents do). The column stays named agent_id for backward
-- compatibility; it is effectively a workload_id.
ALTER TABLE oci_pull_credentials DROP CONSTRAINT IF EXISTS oci_pull_credentials_agent_id_fkey;