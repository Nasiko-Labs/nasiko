-- =============================================================================
-- Replica-hours metering: one row per container/pod run, written by the
-- hours meter reconciler (oss/server/src/agents/hours_meter.rs).
-- agent_id intentionally has NO foreign key: rows must survive hard agent
-- deletion (DELETE FROM agents, catalog/routes.rs) for billing continuity.
-- =============================================================================

CREATE TABLE agent_instance_sessions (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id     UUID NOT NULL,
    agent_name   TEXT NOT NULL,        -- snapshot at first observation
    instance_key TEXT NOT NULL,        -- docker container id / k8s pod uid
    runtime      TEXT NOT NULL,        -- 'docker' | 'kubernetes'
    started_at   TIMESTAMPTZ NOT NULL, -- backend-reported or first-seen fallback
    last_seen_at TIMESTAMPTZ NOT NULL, -- bumped every tick while observed
    ended_at     TIMESTAMPTZ,          -- NULL while running
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT agent_instance_sessions_run_unique UNIQUE (instance_key, started_at)
);

CREATE INDEX idx_agent_instance_sessions_agent_started
    ON agent_instance_sessions (agent_id, started_at);

CREATE INDEX idx_agent_instance_sessions_open
    ON agent_instance_sessions (instance_key) WHERE ended_at IS NULL;
