-- Per-agent compute: the CPU and memory one agent's workload gets, in place of the platform's
-- defaults (a 500m CPU limit, `AGENT_DEFAULT_MEMORY`, and the runtime's own CPU request).
--
-- NULL in any column means the platform default for that one value, which is every existing
-- agent: nothing changes until a value is set. Read by every deploy through the deploy plan
-- (`agent_scaling::load_agent_resources`), so a redeploy, a wake or an update keeps them. Set by
-- `SEED_AGENT_SETTINGS` for the platform's seeded agents.
--
-- `cpu_request_milli` is the CPU the pod is guaranteed (Kubernetes only); with no limit set it is
-- checked against the default limit when the agent deploys, not here.
ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS cpu_limit_milli INT CHECK (cpu_limit_milli > 0),
    ADD COLUMN IF NOT EXISTS cpu_request_milli INT CHECK (cpu_request_milli > 0),
    ADD COLUMN IF NOT EXISTS memory_limit_mib INT CHECK (memory_limit_mib >= 64);

ALTER TABLE agents
    DROP CONSTRAINT IF EXISTS agents_cpu_request_le_limit;
ALTER TABLE agents
    ADD CONSTRAINT agents_cpu_request_le_limit
        CHECK (cpu_request_milli IS NULL OR cpu_limit_milli IS NULL
               OR cpu_request_milli <= cpu_limit_milli);
