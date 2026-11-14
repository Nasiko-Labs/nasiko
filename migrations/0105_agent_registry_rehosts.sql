-- Agents whose stored image moved from one of the registry's former hosts
-- (AGENT_IMAGE_REGISTRY_ALIASES) to its current one and still need a redeploy: a fresh pull
-- credential for the new host and a workload that names it (`agents::rehost`). Queued in the same
-- transaction as the rewrite, removed once the redeploy succeeds, so a crash or a failed deploy is
-- retried instead of leaving an agent that can no longer pull its image.
CREATE TABLE IF NOT EXISTS agent_registry_rehosts (
    agent_id  UUID PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    queued_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
