-- Per-agent environment secrets (encrypted values, injected as env vars on deploy/restart).
ALTER TABLE agents ADD COLUMN secrets_env JSONB NOT NULL DEFAULT '{}';
