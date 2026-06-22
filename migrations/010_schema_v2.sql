-- =============================================================================
-- Migration 010: Schema v2 upgrade
-- Based on Nasiko_DB_Schema.docx (Schema Version 2.0, June 2026)
--
-- This migration is purely ADDITIVE on top of migrations 001–009.
-- It never drops existing columns, never changes existing FK types, and never
-- alters existing NOT NULL constraints without a safe DEFAULT.
--
-- NOTE: agents.id remains UUID (cannot change a PK type without a full table
-- rebuild). All new tables that reference agents use UUID FK accordingly.
--
-- What this migration does
-- ────────────────────────
-- §1  New extension      vector (pgvector)
-- §2  New ENUM types     user_role, auth_provider, artifact_status,
--                        build_status, deployment_status,
--                        upload_pipeline_status, grant_type, chat_role
-- §3  Trigger utility    set_updated_at() + triggers on existing tables
-- §4  departments        new table; deferred FK back-patched onto teams/users
-- §5  teams              + department_id, lead_id, description,
--                          updated_at, deleted_at
-- §6  users              + role, department_id, team_id, role_granted_by,
--                          auth_provider, deleted_at
--                        UPDATE existing is_superuser rows → role = 'admin'
-- §7  New auth tables    user_credentials, user_provider_metadata, auth_tokens
-- §8  agents             + team_id, department_id, is_public,
--                          supports_authenticated_extended_card,
--                          provider_org, provider_url, security JSONB,
--                          deleted_at, search_vector (generated tsvector)
-- §9  New agent tables   agent_skills, agent_grants
-- §10 agent_versions     + status, can_rollback, previous_version
-- §11 agent_builds       + version_id, k8s_job_name, logs, triggered_by
--                        ALTER status TEXT → build_status ENUM
-- §12 agent_deployments  new table
-- §13 upload_status      new table
-- §14 chat               chat_sessions + deleted_at
--                        chat_messages  + message_id, context_id, task_id,
--                                         metadata, has_file_parts
--                        chat_message_files  new table
-- §15 user_n8n_credentials  new table
-- §16 artifacts          new table (pgvector + generated FTS)
-- =============================================================================


-- =============================================================================
-- §1  New extension
-- =============================================================================

CREATE EXTENSION IF NOT EXISTS "vector";   -- pgvector: artifacts.embedding


-- =============================================================================
-- §2  New ENUM types
-- =============================================================================

-- Three-tier org hierarchy role
CREATE TYPE user_role AS ENUM (
    'admin',
    'department_manager',
    'team_lead',
    'team_member',
    'member'
);

-- Authentication provider
CREATE TYPE auth_provider AS ENUM (
    'credentials',
    'github'
);

-- OCI artifact lifecycle
CREATE TYPE artifact_status AS ENUM (
    'preview',
    'active',
    'yanked'
);

-- K8s BuildKit job status (replaces TEXT on agent_builds.status)
CREATE TYPE build_status AS ENUM (
    'queued',
    'building',
    'success',
    'failed'
);

-- K8s Deployment lifecycle
CREATE TYPE deployment_status AS ENUM (
    'starting',
    'running',
    'stopped',
    'failed',
    'crashed'
);

-- Async upload pipeline (7 stages).
-- Named upload_pipeline_status to avoid collision with the upload_status table.
CREATE TYPE upload_pipeline_status AS ENUM (
    'initiated',
    'processing',
    'capabilities_generated',
    'orchestration_triggered',
    'orchestration_processing',
    'completed',
    'failed'
);

-- Access-control grant type (polymorphic grantee)
CREATE TYPE grant_type AS ENUM (
    'user',
    'team',
    'department',
    'agent',
    'public',
    'direct'
);

-- Chat message role
CREATE TYPE chat_role AS ENUM (
    'user',
    'assistant'
);


-- =============================================================================
-- §3  Helper functions
-- =============================================================================

-- array_to_string is STABLE (not IMMUTABLE) due to its polymorphic anyarray
-- signature. GENERATED ALWAYS AS STORED columns require IMMUTABLE expressions.
-- This thin SQL wrapper pins the type to TEXT[] so the planner can treat it as
-- IMMUTABLE. It is safe to mark IMMUTABLE because TEXT[] → TEXT joining on a
-- fixed delimiter is truly referentially transparent.
CREATE OR REPLACE FUNCTION text_array_to_string(arr TEXT[], sep TEXT)
    RETURNS TEXT
    LANGUAGE sql
    IMMUTABLE STRICT PARALLEL SAFE
AS $$
    SELECT array_to_string(arr, sep)
$$;

-- =============================================================================
-- §3  Trigger utility
-- =============================================================================

-- Single BEFORE UPDATE function applied to every table with updated_at.
-- Prevents clock drift caused by application-layer timestamp generation across
-- multiple service instances with slightly different system clocks.
CREATE OR REPLACE FUNCTION set_updated_at()
    RETURNS TRIGGER
    LANGUAGE plpgsql
AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

-- Wire the trigger onto existing tables that already carry updated_at.
-- (New tables created below attach their own trigger inline.)
CREATE TRIGGER trg_users_updated_at
    BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_agents_updated_at
    BEFORE UPDATE ON agents
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_agent_builds_updated_at
    BEFORE UPDATE ON agent_builds
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_user_secrets_updated_at
    BEFORE UPDATE ON user_secrets
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_chat_sessions_updated_at
    BEFORE UPDATE ON chat_sessions
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_model_pricing_updated_at
    BEFORE UPDATE ON model_pricing
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();


-- =============================================================================
-- §4  departments
-- New top-level org table. manager_id is a DEFERRABLE FK to users — the
-- circular reference (departments → users → departments) is resolved by
-- deferring both sides so the hierarchy can bootstrap in one transaction.
-- =============================================================================

CREATE TABLE departments (
    id          UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    name        VARCHAR(255) NOT NULL UNIQUE,
    description TEXT,
    manager_id  UUID,                         -- deferred FK wired below
    settings    JSONB        NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    deleted_at  TIMESTAMPTZ
);

CREATE INDEX idx_departments_manager ON departments(manager_id);
CREATE INDEX idx_departments_live    ON departments(deleted_at) WHERE deleted_at IS NULL;

CREATE TRIGGER trg_departments_updated_at
    BEFORE UPDATE ON departments
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();


-- =============================================================================
-- §5  teams — additive columns
-- =============================================================================

-- Nullable because existing teams have no department yet.
ALTER TABLE teams
    ADD COLUMN department_id UUID REFERENCES departments(id) ON DELETE RESTRICT;

-- Deferred circular FK: teams.lead_id → users (resolved after users is altered)
ALTER TABLE teams ADD COLUMN lead_id UUID;

ALTER TABLE teams ADD COLUMN description TEXT;

ALTER TABLE teams
    ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW();

ALTER TABLE teams ADD COLUMN deleted_at TIMESTAMPTZ;

CREATE INDEX idx_teams_department ON teams(department_id);
CREATE INDEX idx_teams_lead       ON teams(lead_id);

CREATE TRIGGER trg_teams_updated_at
    BEFORE UPDATE ON teams
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();


-- =============================================================================
-- §6  users — additive columns + backfill
-- =============================================================================

-- Org hierarchy role enum (default = member for existing rows)
ALTER TABLE users
    ADD COLUMN role user_role NOT NULL DEFAULT 'member';

-- Promote existing is_superuser = true rows to role = 'admin'
UPDATE users SET role = 'admin' WHERE is_superuser = true;

-- Nullable; existing users have no department/team placement yet
ALTER TABLE users
    ADD COLUMN department_id UUID REFERENCES departments(id) ON DELETE SET NULL;

ALTER TABLE users
    ADD COLUMN team_id UUID REFERENCES teams(id) ON DELETE SET NULL;

-- Audit: who granted this user their role
ALTER TABLE users
    ADD COLUMN role_granted_by UUID REFERENCES users(id) ON DELETE SET NULL;

-- Auth provider enum
ALTER TABLE users
    ADD COLUMN auth_provider auth_provider NOT NULL DEFAULT 'credentials';

-- Soft-delete sentinel
ALTER TABLE users ADD COLUMN deleted_at TIMESTAMPTZ;

CREATE INDEX idx_users_role          ON users(role);
CREATE INDEX idx_users_department    ON users(department_id);
CREATE INDEX idx_users_team          ON users(team_id);
CREATE INDEX idx_users_active        ON users(is_active) WHERE deleted_at IS NULL;
CREATE INDEX idx_users_org_hierarchy ON users(department_id, team_id, role);

-- Now that users exists with its new columns, wire the deferred circular FKs.
ALTER TABLE departments
    ADD CONSTRAINT fk_departments_manager
    FOREIGN KEY (manager_id) REFERENCES users(id)
    ON DELETE SET NULL
    DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE teams
    ADD CONSTRAINT fk_teams_lead
    FOREIGN KEY (lead_id) REFERENCES users(id)
    ON DELETE SET NULL
    DEFERRABLE INITIALLY DEFERRED;


-- =============================================================================
-- §7  New auth tables
-- =============================================================================

-- user_credentials
-- API key + bcrypt hash for credential-auth users. Kept separate so OAuth
-- users carry no NULL credential columns on the users row.
CREATE TABLE user_credentials (
    user_id            UUID         PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    access_key         VARCHAR(255) NOT NULL UNIQUE,
    access_secret_hash TEXT         NOT NULL,   -- bcrypt, never plaintext
    created_at         TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at         TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE TRIGGER trg_user_credentials_updated_at
    BEFORE UPDATE ON user_credentials
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- user_provider_metadata
-- OAuth profile data (GitHub today; extensible via extra JSONB).
-- Kept alongside the existing user_identities table — both are valid.
-- github_id partial-unique index prevents two accounts linking to the same
-- GitHub identity.
CREATE TABLE user_provider_metadata (
    id              UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID         NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    github_id       VARCHAR(255) UNIQUE,
    github_username VARCHAR(255),
    extra           JSONB        NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX idx_user_provider_github
    ON user_provider_metadata(github_id)
    WHERE github_id IS NOT NULL;

CREATE TRIGGER trg_user_provider_metadata_updated_at
    BEFORE UPDATE ON user_provider_metadata
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- auth_tokens
-- JWT session tokens replacing Redis token:* keys.
-- token_hash = SHA-256 of the JWT token_id — never the raw JWT.
-- revoked_at enables instant invalidation before natural expiry.
-- A pg_cron job should purge expired/revoked rows every 15 minutes.
CREATE TABLE auth_tokens (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash VARCHAR(64) NOT NULL UNIQUE,   -- SHA-256 hex
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX idx_auth_tokens_hash ON auth_tokens(token_hash);
CREATE INDEX idx_auth_tokens_user_active
    ON auth_tokens(user_id, expires_at)
    WHERE revoked_at IS NULL;


-- =============================================================================
-- §8  agents — additive columns
-- NOTE: agents.id stays UUID (cannot alter PK type without full table rebuild).
-- =============================================================================

-- Org hierarchy placement (mirrors users.team_id / department_id)
-- owner_team_id is kept for backwards compat; team_id is the v2 equivalent.
ALTER TABLE agents
    ADD COLUMN team_id UUID REFERENCES teams(id) ON DELETE SET NULL;

ALTER TABLE agents
    ADD COLUMN department_id UUID REFERENCES departments(id) ON DELETE SET NULL;

-- Replaces Redis agent:{id}:public flag
ALTER TABLE agents
    ADD COLUMN is_public BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE agents
    ADD COLUMN supports_authenticated_extended_card BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE agents ADD COLUMN provider_org VARCHAR(255);
ALTER TABLE agents ADD COLUMN provider_url TEXT;

-- A2A security field (separate from security_schemes)
ALTER TABLE agents
    ADD COLUMN security JSONB NOT NULL DEFAULT '[]';

-- Soft-delete sentinel
ALTER TABLE agents ADD COLUMN deleted_at TIMESTAMPTZ;

-- Generated FTS column — maintained automatically on every INSERT/UPDATE.
-- Exclude from SELECT lists; use explicit column names in queries.
ALTER TABLE agents
    ADD COLUMN search_vector tsvector
    GENERATED ALWAYS AS (
        to_tsvector(
            'english'::regconfig,
            coalesce(name, '') || ' ' ||
            coalesce(description, '') || ' ' ||
            text_array_to_string(tags, ' ')
        )
    ) STORED;

CREATE INDEX idx_agents_is_public     ON agents(is_public) WHERE is_public = TRUE;
CREATE INDEX idx_agents_owner_public  ON agents(owner_id, is_public);
CREATE INDEX idx_agents_team          ON agents(team_id);
CREATE INDEX idx_agents_department    ON agents(department_id);
CREATE INDEX idx_agents_fts           ON agents USING gin(search_vector);


-- =============================================================================
-- §9  New agent tables
-- =============================================================================

-- agent_skills
-- Normalised skills extracted from agents.skills JSONB.
-- agents.skills JSONB is kept as a denormalised copy for fast card serialisation.
CREATE TABLE agent_skills (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id    UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    skill_key   VARCHAR(255) NOT NULL,
    name        VARCHAR(255) NOT NULL,
    description TEXT         NOT NULL,
    tags        TEXT[]       NOT NULL DEFAULT '{}',
    examples    JSONB        NOT NULL DEFAULT '[]',
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    UNIQUE (agent_id, skill_key)
);

CREATE INDEX idx_agent_skills_tags ON agent_skills USING gin(tags);

-- agent_grants
-- Access-control grants replacing Redis access sets.
-- grant_type is an ENUM; grantee_id is polymorphic (users/teams/departments/
-- agents, or '*' for public grants).
-- CHECK constraint enforces the public sentinel invariant.
CREATE TABLE agent_grants (
    id         UUID       PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id   UUID       NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    grant_type grant_type NOT NULL,
    grantee_id TEXT       NOT NULL,
    granted_by UUID       REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (agent_id, grant_type, grantee_id),
    CONSTRAINT chk_public_sentinel
        CHECK (
            (grant_type = 'public' AND grantee_id = '*') OR
            (grant_type != 'public' AND grantee_id != '*')
        )
);

CREATE INDEX idx_agent_grants_grantee ON agent_grants(grantee_id, grant_type);
CREATE INDEX idx_agent_grants_agent   ON agent_grants(agent_id);


-- =============================================================================
-- §10  agent_versions — additive columns
-- =============================================================================

ALTER TABLE agent_versions
    ADD COLUMN status VARCHAR(20) NOT NULL DEFAULT 'active';

ALTER TABLE agent_versions
    ADD COLUMN can_rollback BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE agent_versions
    ADD COLUMN previous_version VARCHAR(50);

CREATE INDEX idx_agent_versions_status ON agent_versions(agent_id, status);


-- =============================================================================
-- §11  agent_builds — additive columns + status type migration
-- =============================================================================

-- Link a build to a named agent version
ALTER TABLE agent_builds
    ADD COLUMN version_id UUID REFERENCES agent_versions(id) ON DELETE SET NULL;

-- K8s job tracking
ALTER TABLE agent_builds ADD COLUMN k8s_job_name VARCHAR(255);

-- Captured build log (old schema had logs_url; logs stores the content directly)
ALTER TABLE agent_builds ADD COLUMN logs TEXT;

-- Audit: who triggered the build
ALTER TABLE agent_builds
    ADD COLUMN triggered_by UUID REFERENCES users(id) ON DELETE SET NULL;

-- Migrate status column from unconstrained TEXT to the build_status ENUM.
-- Existing values ('queued', 'building', 'success', 'failed') all map cleanly.
-- Must drop the text DEFAULT first; PostgreSQL cannot auto-cast DEFAULT expressions.
ALTER TABLE agent_builds ALTER COLUMN status DROP DEFAULT;
ALTER TABLE agent_builds ALTER COLUMN status TYPE build_status USING status::build_status;
ALTER TABLE agent_builds ALTER COLUMN status SET DEFAULT 'queued'::build_status;

-- Rebuild the existing plain-text status index as an enum-typed one
DROP INDEX IF EXISTS idx_builds_status;
CREATE INDEX idx_agent_builds_status  ON agent_builds(status);
CREATE INDEX idx_agent_builds_queued  ON agent_builds(status) WHERE status = 'queued';
CREATE INDEX idx_agent_builds_recent  ON agent_builds(agent_id, created_at DESC);


-- =============================================================================
-- §12  agent_deployments — new table
-- K8s Deployment lifecycle + crash-loop tracking.
-- ON DELETE RESTRICT on agent_id and build_id preserves the audit trail.
-- =============================================================================

CREATE TABLE agent_deployments (
    id                  UUID              PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id            UUID              NOT NULL REFERENCES agents(id)       ON DELETE RESTRICT,
    build_id            UUID              NOT NULL REFERENCES agent_builds(id) ON DELETE RESTRICT,
    namespace           VARCHAR(255)      NOT NULL DEFAULT 'nasiko-agents',
    replicas            SMALLINT          NOT NULL DEFAULT 1 CHECK (replicas >= 0),
    status              deployment_status NOT NULL DEFAULT 'starting',
    service_url         TEXT,
    k8s_deployment_name VARCHAR(255),
    owner_id            UUID              REFERENCES users(id) ON DELETE SET NULL,
    -- Crash-loop tracking fields (populated by CrashLoopGuardian)
    pod_name            VARCHAR(255),
    crash_reason        TEXT,
    restart_count       INTEGER           NOT NULL DEFAULT 0 CHECK (restart_count >= 0),
    crashed_at          TIMESTAMPTZ,
    last_logs           TEXT,
    created_at          TIMESTAMPTZ       NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ
);

CREATE INDEX idx_agent_deployments_agent   ON agent_deployments(agent_id);
CREATE INDEX idx_agent_deployments_build   ON agent_deployments(build_id);
CREATE INDEX idx_agent_deployments_running ON agent_deployments(status) WHERE status = 'running';
CREATE INDEX idx_agent_deployments_owner   ON agent_deployments(owner_id, created_at DESC);
CREATE INDEX idx_agent_deployments_ns      ON agent_deployments(namespace);


-- =============================================================================
-- §13  upload_status — new table
-- Async 7-stage agent upload pipeline state machine.
-- upload_id is the client-visible correlation ID.
-- agent_id is nullable — populated after the registry entry is created.
-- =============================================================================

CREATE TABLE upload_status (
    id            UUID                   PRIMARY KEY DEFAULT gen_random_uuid(),
    upload_id     VARCHAR(255)           NOT NULL UNIQUE,
    agent_id      UUID                   REFERENCES agents(id) ON DELETE SET NULL,
    agent_name    VARCHAR(255)           NOT NULL,
    status        upload_pipeline_status NOT NULL DEFAULT 'initiated',
    owner_id      UUID                   REFERENCES users(id)  ON DELETE SET NULL,
    error_message TEXT,
    metadata      JSONB                  NOT NULL DEFAULT '{}',
    created_at    TIMESTAMPTZ            NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ            NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_upload_status_agent    ON upload_status(agent_id);
CREATE INDEX idx_upload_status_inflight ON upload_status(status)
    WHERE status NOT IN ('completed', 'failed');
CREATE INDEX idx_upload_status_owner    ON upload_status(owner_id, created_at DESC);
CREATE INDEX idx_upload_status_name     ON upload_status(agent_name, created_at DESC);

CREATE TRIGGER trg_upload_status_updated_at
    BEFORE UPDATE ON upload_status
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();


-- =============================================================================
-- §14  Chat — additive columns + new table
-- =============================================================================

-- chat_sessions: soft-delete support
ALTER TABLE chat_sessions ADD COLUMN deleted_at TIMESTAMPTZ;
CREATE INDEX idx_chat_sessions_recent ON chat_sessions(user_id, created_at DESC);

-- chat_messages: additional v2 fields
-- message_id is the A2A client-supplied idempotency ID; nullable for existing rows.
ALTER TABLE chat_messages ADD COLUMN message_id  VARCHAR(255);
ALTER TABLE chat_messages ADD COLUMN context_id  VARCHAR(255);
ALTER TABLE chat_messages ADD COLUMN task_id     VARCHAR(255);
ALTER TABLE chat_messages ADD COLUMN metadata    JSONB NOT NULL DEFAULT '{}';
ALTER TABLE chat_messages ADD COLUMN has_file_parts BOOLEAN NOT NULL DEFAULT FALSE;

-- chat_message_files
-- File attachments: stores MinIO/S3 URIs only — bytes live in object storage.
-- message_id is a soft UUID reference (no FK) to avoid coupling to chat_messages.
CREATE TABLE chat_message_files (
    id          UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    message_id  UUID         NOT NULL,           -- soft ref → chat_messages.id
    session_id  TEXT         NOT NULL,
    filename    VARCHAR(500) NOT NULL,
    mime_type   VARCHAR(255) NOT NULL,
    size_bytes  BIGINT       NOT NULL,
    storage_uri TEXT         NOT NULL,            -- minio://bucket/path
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_chat_message_files_msg     ON chat_message_files(message_id);
CREATE INDEX idx_chat_message_files_session ON chat_message_files(session_id);


-- =============================================================================
-- §15  user_n8n_credentials — new table
-- Per-user N8N instance credentials (1-to-1 with users).
-- encrypted_api_key is AES-256-GCM encrypted before insert — never plaintext.
-- =============================================================================

CREATE TABLE user_n8n_credentials (
    id                UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id           UUID         NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    connection_name   VARCHAR(255) NOT NULL,
    n8n_url           TEXT         NOT NULL,
    encrypted_api_key TEXT         NOT NULL,
    credential_type   VARCHAR(50)  NOT NULL DEFAULT 'n8n',
    is_active         BOOLEAN      NOT NULL DEFAULT TRUE,
    last_tested       TIMESTAMPTZ,
    created_at        TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at        TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_n8n_creds_active ON user_n8n_credentials(is_active) WHERE is_active = TRUE;
CREATE INDEX idx_n8n_creds_type   ON user_n8n_credentials(credential_type);

CREATE TRIGGER trg_user_n8n_credentials_updated_at
    BEFORE UPDATE ON user_n8n_credentials
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();


-- =============================================================================
-- §16  artifacts — new table
-- OCI artifact catalog from the artifact-registry service.
-- oci_digest is a soft reference to oci_manifests (no hard FK).
-- embedding (vector 1536) supports pgvector cosine-similarity search.
-- search_vector is a GENERATED tsvector for FTS ranking.
-- Exclude both generated columns from SELECT lists (use explicit column names).
-- =============================================================================

CREATE TABLE artifacts (
    id            UUID            PRIMARY KEY DEFAULT gen_random_uuid(),
    owner         VARCHAR(255)    NOT NULL,
    name          VARCHAR(255)    NOT NULL,
    version       VARCHAR(50)     NOT NULL,
    artifact_type VARCHAR(50)     NOT NULL,
    status        artifact_status NOT NULL DEFAULT 'preview',
    description   TEXT,
    metadata      JSONB           NOT NULL DEFAULT '{}',
    oci_digest    VARCHAR(71),                -- soft ref → oci_manifests(digest)
    size_bytes    BIGINT,
    tags          TEXT[]          NOT NULL DEFAULT '{}',
    framework     VARCHAR(50),
    license       VARCHAR(50),
    embedding     vector(1536),              -- pgvector; requires CREATE EXTENSION vector
    search_vector tsvector        GENERATED ALWAYS AS (
                                      to_tsvector(
                                          'english'::regconfig,
                                          coalesce(name, '') || ' ' ||
                                          coalesce(description, '') || ' ' ||
                                          text_array_to_string(tags, ' ')
                                      )
                                  ) STORED,
    created_at    TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    UNIQUE (owner, name, version)
);

CREATE INDEX idx_artifacts_type_status ON artifacts(artifact_type, status);
CREATE INDEX idx_artifacts_framework
    ON artifacts(artifact_type, framework, status, created_at DESC)
    WHERE status != 'yanked';
CREATE INDEX idx_artifacts_framework_created
    ON artifacts(framework, created_at DESC)
    WHERE status != 'yanked';
CREATE INDEX idx_artifacts_owner_name ON artifacts(owner, name);
CREATE INDEX idx_artifacts_tags       ON artifacts USING gin(tags);
CREATE INDEX idx_artifacts_fts        ON artifacts USING gin(search_vector);
-- IVFFlat for approximate nearest-neighbour similarity.
-- lists=100 suits up to ~1M vectors. Switch to HNSW (pgvector 0.5+) at >1M rows.
CREATE INDEX idx_artifacts_embedding
    ON artifacts USING ivfflat (embedding vector_cosine_ops)
    WITH (lists = 100);

CREATE TRIGGER trg_artifacts_updated_at
    BEFORE UPDATE ON artifacts
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
