-- Adds "upload your own MCP server" support: a connector's URL can now come from
-- either a user-typed external address (source_kind='external_url', the existing
-- behavior) or a platform build+deploy (source_kind='uploaded_build', new). Every
-- downstream system (sharing, permissions, tool aggregation, caching) is unaware of
-- this distinction — it only matters for (a) which SSRF policy applies (net.rs) and
-- (b) whether build_status/mcp_connector_builds rows are meaningful.

CREATE TYPE mcp_connector_source_kind AS ENUM ('external_url', 'uploaded_build');

ALTER TABLE mcp_connectors
    ADD COLUMN source_kind mcp_connector_source_kind NOT NULL DEFAULT 'external_url',
    ADD COLUMN build_status TEXT
        CHECK (build_status IN ('pending', 'building', 'running', 'failed') OR build_status IS NULL),
    ADD COLUMN build_secrets_env JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN container_image_tag TEXT;

-- Replace chk_connectors_provider_fields (023_mcp_schema.sql) so a NULL url is
-- allowed only for an uploaded_build row whose build hasn't finished yet. Every
-- existing composio/external_url row shape is accepted exactly as before.
ALTER TABLE mcp_connectors DROP CONSTRAINT chk_connectors_provider_fields;
ALTER TABLE mcp_connectors ADD CONSTRAINT chk_connectors_provider_fields CHECK (
    (provider_type = 'composio' AND auth_config_id IS NOT NULL AND url IS NULL) OR
    (provider_type = 'mcp_server' AND (
        (source_kind = 'external_url' AND url IS NOT NULL AND auth_config_id IS NULL) OR
        (source_kind = 'uploaded_build' AND auth_config_id IS NULL AND (
            (build_status IN ('pending', 'building', 'failed')) OR
            (build_status = 'running' AND url IS NOT NULL)
        ))
    ))
);

-- Build history for uploaded MCP servers — mirrors agent_builds (0001_schema.sql)
-- exactly: one row per build attempt, mcp_connectors itself only ever reflects the
-- latest state.
CREATE TABLE mcp_connector_builds (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    connector_id     UUID NOT NULL REFERENCES mcp_connectors(id) ON DELETE CASCADE,
    owner_id         UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    version_tag      TEXT NOT NULL,
    github_url       TEXT,
    source_key       TEXT, -- S3 zip path; NULL when github_url is set
    image_tag        TEXT,
    detected_runtime TEXT, -- diagnostic only: 'python' | 'node' | 'unknown', from validation.rs
    status           TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'building', 'success', 'failed')),
    error_msg        TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at     TIMESTAMPTZ
);
CREATE INDEX idx_mcp_connector_builds_connector ON mcp_connector_builds(connector_id, created_at DESC);
CREATE TRIGGER trg_mcp_connector_builds_updated_at
    BEFORE UPDATE ON mcp_connector_builds FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- build_jobs.agent_id is NOT NULL with a hard FK to agents(id) (0001_schema.sql) — an
-- MCP-server build job has no agents row to reference. Widen the queue additively
-- (no rename, no unrelated refactor to the many existing agent_id call sites):
-- agent_id becomes optional, a new optional connector_id sits alongside it, and a
-- CHECK guarantees every job still references exactly one real target.
ALTER TABLE build_jobs
    ALTER COLUMN agent_id DROP NOT NULL,
    ADD COLUMN connector_id UUID REFERENCES mcp_connectors(id) ON DELETE CASCADE,
    ADD CONSTRAINT chk_build_jobs_one_target CHECK (
        (agent_id IS NOT NULL AND connector_id IS NULL) OR
        (agent_id IS NULL AND connector_id IS NOT NULL)
    );
CREATE INDEX idx_build_jobs_connector ON build_jobs(connector_id) WHERE connector_id IS NOT NULL;
