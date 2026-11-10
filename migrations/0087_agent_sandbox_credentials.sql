-- The credentials a parked agent's sandbox carries.
--
-- A sandbox's environment is fixed when it is created and a resume (Docker) keeps it, while the
-- platform's gateway and state credentials are single-live-token: minting one supersedes the
-- previous (kept valid for a 15-minute grace). Minting on every wake would therefore break a
-- sandbox's own tokens by its third request. The minted values are minted once and kept here,
-- encrypted with the platform master key (the agent's sandbox-token scope, the agent id as
-- associated data), and handed back on every later acquire. Absent = not minted yet, or cleared
-- because the agent was updated or its sandbox released; the next wake mints afresh.
CREATE TABLE IF NOT EXISTS agent_sandbox_credentials (
    agent_id   UUID PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    env_enc    BYTEA NOT NULL,
    minted_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
