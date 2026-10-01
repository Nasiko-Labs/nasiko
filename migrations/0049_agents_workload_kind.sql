-- Persist an agent's workload kind on the catalog row.
--
-- The agent proxy needs to know, at request time, whether an agent is an
-- ordinary HTTP/A2A agent or an ACP coding agent — the two are driven
-- completely differently (reverse-proxy an HTTP endpoint vs. open a stdio turn).
-- Until now `WorkloadKind` existed only at the runtime layer and was never
-- stored, so the proxy had no way to tell them apart. This adds the column it
-- branches on.
--
-- Defaults to 'agent' so every existing row keeps today's behavior; the values
-- mirror the runtime `WorkloadKind` variants.
ALTER TABLE agents
    ADD COLUMN workload_kind TEXT NOT NULL DEFAULT 'agent'
        CHECK (workload_kind IN ('agent', 'mcp_connector', 'acp_coding_agent'));
