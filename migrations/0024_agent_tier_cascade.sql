-- Smart tier cascade vs stick-to-selected-provider (agent BYOK / pinned config).
-- When true (default): POST /v1/route uses the platform DronaHQ tier cascade
-- (groq → mistral → openai → nvidia) with platform keys — ignores agent llm_config.
-- When false: /v1/route with an agent JWT resolves the agent's llm_config / BYOK
-- provider (e.g. NVIDIA) and calls that single destination.
ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS tier_cascade BOOLEAN NOT NULL DEFAULT true;

COMMENT ON COLUMN agents.tier_cascade IS
    'true = /v1/route platform tier cascade; false = honor agent llm_config / BYOK provider';
