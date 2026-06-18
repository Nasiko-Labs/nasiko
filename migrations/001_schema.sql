CREATE EXTENSION IF NOT EXISTS "pgcrypto";
CREATE EXTENSION IF NOT EXISTS "pg_trgm";

-- =============================================================================
-- Users
-- =============================================================================

CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    username TEXT NOT NULL UNIQUE,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT,
    display_name TEXT,
    avatar_url TEXT,
    is_superuser BOOLEAN NOT NULL DEFAULT false,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_by UUID REFERENCES users(id),
    last_login TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- =============================================================================
-- User Identities (OAuth/SSO providers linked to a user)
-- =============================================================================

CREATE TABLE user_identities (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    provider_username TEXT,
    provider_metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(provider, provider_id)
);

CREATE INDEX idx_user_identities_user ON user_identities(user_id);

-- =============================================================================
-- Teams
-- =============================================================================

CREATE TABLE teams (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL UNIQUE,
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- =============================================================================
-- Team Membership
-- =============================================================================

CREATE TYPE team_role AS ENUM ('owner', 'admin', 'deployer', 'viewer');

CREATE TABLE team_members (
    team_id UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role team_role NOT NULL DEFAULT 'viewer',
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (team_id, user_id)
);

CREATE INDEX idx_team_members_user ON team_members(user_id);

-- =============================================================================
-- Agent Registry (agent cards)
-- =============================================================================

CREATE TABLE agents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL UNIQUE,
    display_name TEXT,
    description TEXT,
    owner_id UUID NOT NULL REFERENCES users(id),
    owner_team_id UUID NOT NULL REFERENCES teams(id),
    url TEXT,
    icon_url TEXT,
    version TEXT NOT NULL DEFAULT '1.0.0',
    protocol_version TEXT NOT NULL DEFAULT '0.2.9',
    preferred_transport TEXT NOT NULL DEFAULT 'JSONRPC',
    documentation_url TEXT,
    capabilities JSONB NOT NULL DEFAULT '{"streaming": false, "pushNotifications": false, "stateTransitionHistory": false, "chat_agent": false}',
    security_schemes JSONB NOT NULL DEFAULT '{}',
    default_input_modes JSONB NOT NULL DEFAULT '["application/json", "text/plain"]',
    default_output_modes JSONB NOT NULL DEFAULT '["application/json"]',
    skills JSONB NOT NULL DEFAULT '[]',
    tags TEXT[] NOT NULL DEFAULT '{}',
    metadata JSONB NOT NULL DEFAULT '{}',
    status TEXT NOT NULL DEFAULT 'registered',
    image TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_agents_owner ON agents(owner_id);
CREATE INDEX idx_agents_owner_team ON agents(owner_team_id);
CREATE INDEX idx_agents_status ON agents(status);
CREATE INDEX idx_agents_name_trgm ON agents USING gin(name gin_trgm_ops);
CREATE INDEX idx_agents_tags ON agents USING gin(tags);

-- =============================================================================
-- Agent Builds
-- =============================================================================

CREATE TABLE agent_builds (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    github_url TEXT,
    commit_hash TEXT,
    version_tag TEXT NOT NULL,
    image_reference TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    logs_url TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_builds_agent ON agent_builds(agent_id);
CREATE INDEX idx_builds_status ON agent_builds(status);

-- =============================================================================
-- Agent Versions (successful builds become versions)
-- =============================================================================

CREATE TABLE agent_versions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    build_id UUID REFERENCES agent_builds(id),
    version TEXT NOT NULL,
    image_tag TEXT NOT NULL,
    changelog TEXT,
    is_active BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(agent_id, version)
);

CREATE INDEX idx_versions_active ON agent_versions(agent_id) WHERE is_active = true;

-- =============================================================================
-- Cross-team agent access
-- Agents within the same team can access each other by default.
-- This table grants access to agents from OTHER teams.
-- =============================================================================

CREATE TABLE cross_team_agent_access (
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    accessor_team_id UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    permission TEXT NOT NULL DEFAULT 'use',
    granted_by UUID REFERENCES users(id),
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (agent_id, accessor_team_id)
);

-- =============================================================================
-- Secrets (encrypted env vars per user)
-- =============================================================================

CREATE TABLE user_secrets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    encrypted_value TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(user_id, name)
);

CREATE INDEX idx_secrets_user ON user_secrets(user_id);

-- =============================================================================
-- Chat Sessions & History
-- =============================================================================

CREATE TABLE chat_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id TEXT NOT NULL UNIQUE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    agent_url TEXT,
    title TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_sessions_user ON chat_sessions(user_id);
CREATE INDEX idx_sessions_created ON chat_sessions(created_at DESC);

CREATE TABLE chat_messages (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id TEXT NOT NULL REFERENCES chat_sessions(session_id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    file_parts JSONB,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_messages_session ON chat_messages(session_id, timestamp);

-- =============================================================================
-- OCI Registry (artifact storage metadata)
-- =============================================================================

CREATE TABLE oci_manifests (
    digest TEXT PRIMARY KEY,
    repository TEXT NOT NULL,
    reference TEXT,
    media_type TEXT NOT NULL,
    content TEXT NOT NULL,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_manifests_repo ON oci_manifests(repository);
CREATE INDEX idx_manifests_repo_ref ON oci_manifests(repository, reference);

CREATE TABLE oci_uploads (
    uuid UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    repository TEXT NOT NULL,
    offset_bytes BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE oci_referrers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    subject_digest TEXT NOT NULL,
    repository TEXT NOT NULL,
    referrer_digest TEXT NOT NULL,
    artifact_type TEXT,
    annotations JSONB,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(subject_digest, referrer_digest)
);

CREATE INDEX idx_referrers_subject ON oci_referrers(repository, subject_digest);

-- =============================================================================
-- Proxy Audit Logs
-- =============================================================================

CREATE TABLE proxy_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    caller_id UUID NOT NULL REFERENCES users(id),
    target_agent_id UUID NOT NULL REFERENCES agents(id),
    method TEXT NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT now(),
    latency_ms BIGINT NOT NULL,
    status INTEGER NOT NULL,
    error TEXT,
    metadata JSONB DEFAULT '{}'
);

CREATE INDEX idx_proxy_logs_caller ON proxy_logs(caller_id, timestamp DESC);
CREATE INDEX idx_proxy_logs_target ON proxy_logs(target_agent_id, timestamp DESC);
CREATE INDEX idx_proxy_logs_timestamp ON proxy_logs(timestamp DESC);
