-- Index support for two access paths 0056 left to sequential scans: the
-- ON DELETE SET NULL action on chat_message_files.workspace_file_id (fired once
-- per workspace_files row removed when a user or agent is hard-deleted) and the
-- per-request capability lookup on relay transfers. Both partial, so they stay
-- small regardless of history.
CREATE INDEX idx_chat_message_files_workspace_file
    ON chat_message_files(workspace_file_id) WHERE workspace_file_id IS NOT NULL;
CREATE INDEX idx_workspace_uploads_capability
    ON workspace_uploads(capability_hash) WHERE state = 'pending';
