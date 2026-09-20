-- =============================================================================
-- LLM routing: reusable LLM configs, the tier→model registry, the Thompson-
-- sampling quality cells, and the agent-side columns that tie in.
-- =============================================================================

-- =============================================================================
-- llm_configs — reusable per-user LLM configs (library)
-- =============================================================================
-- A user creates named configs once and attaches/reuses them across their own
-- agents. Resolution for an agent is:
--   attached config → the agent owner's default config → NULL (platform
--   defaults: DEFAULT_PROVIDER / DEFAULT_MODEL and the platform API key).
-- Ownership is per-user (`created_by`); a user can attach only their own
-- configs to their own agents, so owner == config creator == secret owner and
-- the resolver's per-user secret path (SecretsCrypto::for_user) is unchanged.
CREATE TABLE llm_configs (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    created_by          UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name                TEXT NOT NULL,
    provider            TEXT NOT NULL,
    -- Optional: users can rely entirely on tier-based routing (tier*_model
    -- below or the global model_registry) instead of pinning one model.
    model               TEXT,
    fallback_models     JSONB NOT NULL DEFAULT '[]'::jsonb,
    temperature         DOUBLE PRECISION,
    max_tokens          BIGINT,
    api_key_secret_name TEXT,
    pinned              BOOLEAN NOT NULL DEFAULT false,
    pinned_model        TEXT,
    is_default          BOOLEAN NOT NULL DEFAULT false,
    -- Per-config tier→model overrides. When set, the smart router uses these
    -- instead of the global model_registry for this config's provider.
    -- NULL = fall through to the global registry.
    tier1_model         TEXT,
    tier2_model         TEXT,
    tier3_model         TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at          TIMESTAMPTZ
);

-- Config names are unique per owner (active rows only, so a deleted name is reusable).
CREATE UNIQUE INDEX uq_llm_configs_owner_name
    ON llm_configs (created_by, name) WHERE deleted_at IS NULL;

-- At most one default per owner.
CREATE UNIQUE INDEX uq_llm_configs_owner_default
    ON llm_configs (created_by) WHERE is_default AND deleted_at IS NULL;

CREATE INDEX idx_llm_configs_owner ON llm_configs (created_by) WHERE deleted_at IS NULL;

CREATE TRIGGER trg_llm_configs_updated_at BEFORE UPDATE ON llm_configs
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- =============================================================================
-- Agent-side LLM columns
-- =============================================================================

-- Agents reference a config instead of embedding one.
ALTER TABLE agents ADD COLUMN llm_config_id UUID REFERENCES llm_configs(id);
CREATE INDEX idx_agents_llm_config_id ON agents (llm_config_id) WHERE llm_config_id IS NOT NULL;

-- Per-agent inbound SDK format: which LLM SDK the agent's *code* speaks, so
-- the deploy-time injector writes the matching base-URL/key env vars (OpenAI →
-- OPENAI_*, Anthropic → ANTHROPIC_*, Gemini → GOOGLE_*/GEMINI_*). Independent
-- of the *outbound* provider in the attached llm_config (where we route the
-- call). Defaults to 'openai'.
ALTER TABLE agents
    ADD COLUMN inbound_format TEXT NOT NULL DEFAULT 'openai'
    CHECK (inbound_format IN ('openai', 'anthropic', 'gemini'));

-- Per-agent model pin. Overrides the config-level pin. Cleared automatically
-- when the agent's llm_config_id changes (the pin was set in the context of
-- the previous config).
ALTER TABLE agents ADD COLUMN pinned_model TEXT;

-- =============================================================================
-- model_registry — tier→model registry for the smart model router (S2)
-- =============================================================================
-- The query classifier picks a coarse strength Tier (1 = strongest … 3 =
-- smallest); the router then looks up the concrete model for (destination
-- provider, tier) here. This is independent of agents.llm_config_id (which
-- fixes the *provider*/key) — the registry only decides *which model* of that
-- provider a classified request uses.
--
-- The router (PgTierRegistry) reads this table and falls back to compiled-in
-- static seeds on a missing row or a DB error, so an absent/unreachable
-- registry never breaks routing. These seed rows MUST mirror
-- StaticTierRegistry::seed in oss/llm-router.
CREATE TABLE model_registry (
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

-- Seeds mirror the static table. ON CONFLICT keeps re-application idempotent.
INSERT INTO model_registry (provider, tier, model) VALUES
    ('anthropic', 1, 'claude-opus-4-8'),
    ('anthropic', 2, 'claude-sonnet-4-6'),
    ('anthropic', 3, 'claude-haiku-4-5'),
    ('openai', 1, 'gpt-5.5'),
    ('openai', 2, 'gpt-5.4'),
    ('openai', 3, 'gpt-4o-mini')
ON CONFLICT (provider, tier) DO NOTHING;

-- =============================================================================
-- router_quality_cells — Thompson-sampling tier selection memory (S5)
-- =============================================================================
-- The classifier buckets each query into a request type, then treats the
-- three strength tiers as bandit arms and Thompson-samples one from a Beta
-- posterior. This table is that posterior's memory: for a (provider, tier,
-- request_type) it stores a running mean of the observed reward and how many
-- observations back it. Rewards come from the user's next-turn reaction
-- (approval → 1.0, complaint → 0.0), credited by route_model.
--
-- Cells are scoped PER PROVIDER (tiers/costs are provider-specific) and shared
-- across conversations, so learning from one conversation improves the next
-- one's cold-start pick.
--
-- The router (PgCellStore) treats this as a latency/quality optimisation,
-- never a correctness dependency: a read failure degrades to cold-start
-- priors (an empty cell set) and a write failure is dropped, so an
-- absent/unreachable table stalls learning but never breaks routing. Starts
-- empty — there are no seed rows; every cell is earned from feedback.
CREATE TABLE router_quality_cells (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider TEXT NOT NULL,
    tier SMALLINT NOT NULL CHECK (tier IN (1, 2, 3)),
    request_type TEXT NOT NULL,
    -- Running mean of observed reward in [0, 1]; see update_cell in oss/llm-router.
    quality_mean DOUBLE PRECISION NOT NULL,
    -- Effective sample count, capped by the router at MAX_SAMPLES (200).
    samples BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (provider, tier, request_type)
);

-- The (provider) prefix of the unique index already serves PgCellStore's per-provider load.

CREATE TRIGGER trg_router_quality_cells_updated_at BEFORE UPDATE ON router_quality_cells
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
