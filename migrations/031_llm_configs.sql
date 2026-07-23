-- Reusable per-user LLM configs (library). Replaces the per-agent inline
-- `agents.llm_config` JSONB (migration 030): a user creates named configs once and
-- attaches/reuses them across their own agents. Resolution for an agent is:
--   attached config → the agent owner's default config → NULL (platform defaults).
-- Ownership is per-user (`created_by`); a user can attach only their own configs to
-- their own agents, so owner == config creator == secret owner and the resolver's
-- existing per-user secret path (SecretsCrypto::for_user) is unchanged.

CREATE TABLE llm_configs (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    created_by          UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name                TEXT NOT NULL,
    provider            TEXT NOT NULL,
    model               TEXT NOT NULL,
    fallback_models     JSONB NOT NULL DEFAULT '[]'::jsonb,
    temperature         DOUBLE PRECISION,
    max_tokens          BIGINT,
    api_key_secret_name TEXT,
    pinned              BOOLEAN NOT NULL DEFAULT false,
    pinned_model        TEXT,
    is_default          BOOLEAN NOT NULL DEFAULT false,
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

-- Agents now reference a config instead of embedding one.
ALTER TABLE agents ADD COLUMN llm_config_id UUID REFERENCES llm_configs(id);
CREATE INDEX idx_agents_llm_config_id ON agents (llm_config_id) WHERE llm_config_id IS NOT NULL;

-- Migrate every existing inline config (migration 030) into a library row, then link
-- the agent to it. Only active agents with a well-formed config are migrated; a
-- soft-deleted agent doesn't route, and would risk a name collision on the unique
-- index (its name may be reused by an active agent).
INSERT INTO llm_configs (created_by, name, provider, model, fallback_models,
                         temperature, max_tokens, api_key_secret_name, pinned, pinned_model)
SELECT a.owner_id,
       a.name || '-config',
       a.llm_config->>'provider',
       a.llm_config->>'model',
       COALESCE(a.llm_config->'fallback_models', '[]'::jsonb),
       (a.llm_config->>'temperature')::double precision,
       (a.llm_config->>'max_tokens')::bigint,
       a.llm_config->>'api_key_secret_name',
       COALESCE((a.llm_config->>'pinned')::boolean, false),
       a.llm_config->>'pinned_model'
FROM agents a
WHERE a.deleted_at IS NULL
  AND a.llm_config IS NOT NULL
  AND a.llm_config->>'provider' IS NOT NULL
  AND a.llm_config->>'model' IS NOT NULL;

UPDATE agents a
SET llm_config_id = c.id
FROM llm_configs c
WHERE a.deleted_at IS NULL
  AND a.llm_config IS NOT NULL
  AND c.created_by = a.owner_id
  AND c.name = a.name || '-config';

ALTER TABLE agents DROP COLUMN llm_config;
