-- Per-config tier→model overrides. When set, the smart router uses these instead
-- of the global model_registry for this config's provider. NULL = fall through to
-- the global registry (the current behaviour).
ALTER TABLE llm_configs
    ADD COLUMN IF NOT EXISTS tier1_model TEXT,
    ADD COLUMN IF NOT EXISTS tier2_model TEXT,
    ADD COLUMN IF NOT EXISTS tier3_model TEXT;

-- model is now optional — users can rely entirely on tier-based routing.
ALTER TABLE llm_configs ALTER COLUMN model DROP NOT NULL;