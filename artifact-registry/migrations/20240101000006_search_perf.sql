-- Compound index covering the common search pattern:
--   WHERE artifact_type = ? AND framework = ? AND status != 'yanked' ORDER BY created_at DESC
-- Previously only (artifact_type, status) existed; framework had no index → heap scan.
CREATE INDEX IF NOT EXISTS artifacts_type_fw_status_created
    ON artifacts(artifact_type, framework, status, created_at DESC)
    WHERE status != 'yanked';

-- Index on framework alone for queries that filter by framework without type
CREATE INDEX IF NOT EXISTS artifacts_framework_created
    ON artifacts(framework, created_at DESC)
    WHERE status != 'yanked';
