-- Per-agent model pin. Overrides the config-level pin. Cleared automatically
-- when the agent's llm_config_id changes (the pin was set in the context of
-- the previous config).
ALTER TABLE agents ADD COLUMN IF NOT EXISTS pinned_model TEXT;