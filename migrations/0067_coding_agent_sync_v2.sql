-- Sync progress v2: a run reports the history windows it reads and is `sending` while queued turns
-- are still being delivered; the user can ask a device to re-send the turns it quarantined.

-- Read windows as the client last reported them (newest first), validated by the server.
ALTER TABLE coding_agent_sync_runs
    ADD COLUMN windows JSONB NOT NULL DEFAULT '[]'::jsonb,
    -- When the user last asked this device to retry its quarantined turns; NULL if never.
    ADD COLUMN retry_requested_at TIMESTAMPTZ;

ALTER TABLE coding_agent_sync_runs DROP CONSTRAINT IF EXISTS coding_agent_sync_runs_phase_check;
ALTER TABLE coding_agent_sync_runs ADD CONSTRAINT coding_agent_sync_runs_phase_check CHECK (phase IN (
    'awaiting_consent', 'discovering', 'connecting', 'importing', 'sending', 'done', 'failed'
));

-- Consent moved to the local client: running `nasiko agents setup` is the answer, and the desktop
-- app stores its own. The server-side answer 0066 introduced has no reader or writer left.
DROP TABLE IF EXISTS coding_agent_consent;
