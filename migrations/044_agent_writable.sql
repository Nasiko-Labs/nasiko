-- Persists the `--writable` deploy flag so redeploy/update/rollback paths can
-- restore it without the caller having to pass it again on every call.
-- Defaults to false — backward compatible for every existing agent.
ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS writable BOOLEAN NOT NULL DEFAULT false;
