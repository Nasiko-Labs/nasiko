-- Tier→model registry for the smart model router (S2).
--
-- The query classifier picks a coarse strength Tier (1 = strongest … 3 = smallest); the
-- router then looks up the concrete model for (destination provider, tier) here. This is
-- independent of agents.llm_config (which fixes the *provider*/key) — the registry only
-- decides *which model* of that provider a classified request uses.
--
-- The router (PgTierRegistry) reads this table and falls back to compiled-in static seeds
-- on a missing row or a DB error, so an absent/unreachable registry never breaks routing.
-- These seed rows MUST mirror StaticTierRegistry::seed in oss/llm-router.
CREATE TABLE IF NOT EXISTS model_registry (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider TEXT NOT NULL,
    tier SMALLINT NOT NULL CHECK (tier IN (1, 2, 3)),
    model TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (provider, tier)
);
CREATE TRIGGER trg_model_registry_updated_at BEFORE UPDATE ON model_registry
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Seeds mirror the static table. ON CONFLICT keeps re-application idempotent (unlike the
-- 012 pricing seed — see KNOWN_ISSUES KI-3).
INSERT INTO model_registry (provider, tier, model) VALUES
    ('anthropic', 1, 'claude-opus-4-8'),
    ('anthropic', 2, 'claude-sonnet-4-6'),
    ('anthropic', 3, 'claude-haiku-4-5'),
    ('openai', 1, 'gpt-5.5'),
    ('openai', 2, 'gpt-5.4'),
    ('openai', 3, 'gpt-4o-mini')
ON CONFLICT (provider, tier) DO NOTHING;
