-- A user's own config for an agent someone else owns: the user's calls through that agent run on it,
-- with the user's key. `llm_configs` rows are soft-deleted, so readers join on a live row the user created;
-- the cascades only cover hard deletes.
CREATE TABLE agent_user_llm_configs (
    user_id       UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    agent_id      UUID        NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    llm_config_id UUID        NOT NULL REFERENCES llm_configs(id) ON DELETE CASCADE,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, agent_id)
);
