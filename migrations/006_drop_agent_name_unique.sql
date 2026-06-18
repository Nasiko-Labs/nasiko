-- Agent names don't need to be globally unique.
-- Deploy uses agent ID (stored in .nasiko/agent.json) for updates.
ALTER TABLE agents DROP CONSTRAINT agents_name_key;
