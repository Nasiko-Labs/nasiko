CREATE EXTENSION IF NOT EXISTS "pgcrypto";
CREATE EXTENSION IF NOT EXISTS "pg_trgm";
CREATE EXTENSION IF NOT EXISTS "vector";

-- Helper: immutable wrapper for GENERATED columns
CREATE OR REPLACE FUNCTION text_array_to_string(arr TEXT[], sep TEXT)
    RETURNS TEXT LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
AS $$ SELECT array_to_string(arr, sep) $$;

-- Helper: auto-update updated_at on row change
CREATE OR REPLACE FUNCTION set_updated_at() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN NEW.updated_at = NOW(); RETURN NEW; END; $$;

-- ENUMs
CREATE TYPE user_role AS ENUM ('admin', 'member');
CREATE TYPE grant_type AS ENUM ('user', 'public');
CREATE TYPE artifact_status AS ENUM ('preview', 'active', 'yanked');
CREATE TYPE build_status AS ENUM ('queued', 'building', 'success', 'failed');
CREATE TYPE deployment_status AS ENUM ('starting', 'running', 'stopped', 'failed', 'crashed');
CREATE TYPE upload_pipeline_status AS ENUM ('initiated', 'processing', 'capabilities_generated', 'orchestration_triggered', 'orchestration_processing', 'completed', 'failed');
CREATE TYPE chat_role AS ENUM ('user', 'assistant');

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
    role user_role NOT NULL DEFAULT 'member',
    is_superuser BOOLEAN NOT NULL DEFAULT false,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_by UUID REFERENCES users(id),
    last_login TIMESTAMPTZ,
    deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_users_role ON users(role);
CREATE INDEX idx_users_active ON users(is_active) WHERE deleted_at IS NULL;
CREATE TRIGGER trg_users_updated_at BEFORE UPDATE ON users FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- User identities (OAuth/SSO providers)
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

-- User credentials (API key auth)
CREATE TABLE user_credentials (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    access_key VARCHAR(255) NOT NULL UNIQUE,
    access_secret_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TRIGGER trg_user_credentials_updated_at BEFORE UPDATE ON user_credentials FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Auth tokens (JWT session tracking)
CREATE TABLE auth_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash VARCHAR(64) NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_auth_tokens_user_active ON auth_tokens(user_id, expires_at) WHERE revoked_at IS NULL;

-- User secrets (encrypted env vars)
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
CREATE TRIGGER trg_user_secrets_updated_at BEFORE UPDATE ON user_secrets FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- =============================================================================
-- Agents
-- =============================================================================

CREATE TABLE agents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    display_name TEXT,
    description TEXT,
    owner_id UUID NOT NULL REFERENCES users(id),
    url TEXT,
    icon_url TEXT,
    version TEXT NOT NULL DEFAULT '1.0.0',
    protocol_version TEXT NOT NULL DEFAULT '0.2.9',
    preferred_transport TEXT NOT NULL DEFAULT 'JSONRPC',
    documentation_url TEXT,
    capabilities JSONB NOT NULL DEFAULT '{"streaming": false, "pushNotifications": false, "stateTransitionHistory": false, "chat_agent": false}',
    security_schemes JSONB NOT NULL DEFAULT '{}',
    security JSONB NOT NULL DEFAULT '[]',
    default_input_modes JSONB NOT NULL DEFAULT '["application/json", "text/plain"]',
    default_output_modes JSONB NOT NULL DEFAULT '["application/json"]',
    skills JSONB NOT NULL DEFAULT '[]',
    tags TEXT[] NOT NULL DEFAULT '{}',
    metadata JSONB NOT NULL DEFAULT '{}',
    secrets_env JSONB NOT NULL DEFAULT '{}',
    status TEXT NOT NULL DEFAULT 'registered',
    image TEXT,
    is_public BOOLEAN NOT NULL DEFAULT false,
    supports_authenticated_extended_card BOOLEAN NOT NULL DEFAULT false,
    provider_org VARCHAR(255),
    provider_url TEXT,
    deleted_at TIMESTAMPTZ,
    search_vector tsvector GENERATED ALWAYS AS (
        to_tsvector('english'::regconfig,
            coalesce(name, '') || ' ' || coalesce(description, '') || ' ' || text_array_to_string(tags, ' ')
        )
    ) STORED,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_agents_owner ON agents(owner_id);
CREATE INDEX idx_agents_status ON agents(status);
CREATE INDEX idx_agents_name_trgm ON agents USING gin(name gin_trgm_ops);
CREATE INDEX idx_agents_tags ON agents USING gin(tags);
CREATE INDEX idx_agents_is_public ON agents(is_public) WHERE is_public = true;
CREATE INDEX idx_agents_owner_public ON agents(owner_id, is_public);
CREATE INDEX idx_agents_fts ON agents USING gin(search_vector);
CREATE TRIGGER trg_agents_updated_at BEFORE UPDATE ON agents FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Agent-to-agent ACL (allowlist: no rows = unrestricted)
CREATE TABLE agent_acl (
    caller_agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    target_agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    granted_by UUID REFERENCES users(id),
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (caller_agent_id, target_agent_id)
);
CREATE INDEX idx_agent_acl_caller ON agent_acl(caller_agent_id);

-- Agent grants (owner-controlled access)
CREATE TABLE agent_grants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    grant_type grant_type NOT NULL,
    grantee_id TEXT NOT NULL,
    granted_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (agent_id, grant_type, grantee_id),
    CONSTRAINT chk_public_sentinel CHECK (
        (grant_type = 'public' AND grantee_id = '*') OR
        (grant_type != 'public' AND grantee_id != '*')
    )
);
CREATE INDEX idx_agent_grants_grantee ON agent_grants(grantee_id, grant_type);
CREATE INDEX idx_agent_grants_agent ON agent_grants(agent_id);

-- Agent skills (normalised from agents.skills JSONB)
CREATE TABLE agent_skills (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    skill_key VARCHAR(255) NOT NULL,
    name VARCHAR(255) NOT NULL,
    description TEXT NOT NULL,
    tags TEXT[] NOT NULL DEFAULT '{}',
    examples JSONB NOT NULL DEFAULT '[]',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (agent_id, skill_key),
    CONSTRAINT tags_lowercase CHECK (tags = ARRAY(SELECT lower(t) FROM unnest(tags) AS t))
);
CREATE INDEX idx_agent_skills_tags ON agent_skills USING gin(tags);

-- =============================================================================
-- Builds & Deployments
-- =============================================================================

CREATE TABLE agent_builds (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    github_url TEXT,
    commit_hash TEXT,
    version_tag TEXT NOT NULL,
    image_reference TEXT NOT NULL,
    status build_status NOT NULL DEFAULT 'queued',
    logs_url TEXT,
    logs TEXT,
    k8s_job_name VARCHAR(255),
    triggered_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_builds_agent ON agent_builds(agent_id);
CREATE INDEX idx_agent_builds_status ON agent_builds(status);
CREATE INDEX idx_agent_builds_queued ON agent_builds(status) WHERE status = 'queued';
CREATE INDEX idx_agent_builds_recent ON agent_builds(agent_id, created_at DESC);
CREATE TRIGGER trg_agent_builds_updated_at BEFORE UPDATE ON agent_builds FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE agent_versions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    build_id UUID REFERENCES agent_builds(id),
    version TEXT NOT NULL,
    image_tag TEXT NOT NULL,
    changelog TEXT,
    is_active BOOLEAN NOT NULL DEFAULT false,
    status VARCHAR(20) NOT NULL DEFAULT 'active',
    can_rollback BOOLEAN NOT NULL DEFAULT false,
    previous_version VARCHAR(50),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(agent_id, version)
);
CREATE INDEX idx_versions_active ON agent_versions(agent_id) WHERE is_active = true;
CREATE INDEX idx_agent_versions_status ON agent_versions(agent_id, status);

-- version_id FK added after agent_versions exists
ALTER TABLE agent_builds ADD COLUMN version_id UUID REFERENCES agent_versions(id) ON DELETE SET NULL;

CREATE TABLE agent_deployments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    build_id UUID NOT NULL REFERENCES agent_builds(id) ON DELETE RESTRICT,
    namespace VARCHAR(255) NOT NULL DEFAULT 'nasiko-agents',
    replicas SMALLINT NOT NULL DEFAULT 1 CHECK (replicas >= 0),
    status deployment_status NOT NULL DEFAULT 'starting',
    service_url TEXT,
    k8s_deployment_name VARCHAR(255),
    owner_id UUID REFERENCES users(id) ON DELETE SET NULL,
    spec_ports INTEGER[] DEFAULT '{8000}',
    spec_image TEXT,
    pod_name VARCHAR(255),
    crash_reason TEXT,
    restart_count INTEGER NOT NULL DEFAULT 0 CHECK (restart_count >= 0),
    crashed_at TIMESTAMPTZ,
    last_logs TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ
);
CREATE INDEX idx_agent_deployments_agent ON agent_deployments(agent_id);
CREATE INDEX idx_agent_deployments_build ON agent_deployments(build_id);
CREATE INDEX idx_agent_deployments_running ON agent_deployments(status) WHERE status = 'running';
CREATE INDEX idx_agent_deployments_owner ON agent_deployments(owner_id, created_at DESC);
CREATE INDEX idx_agent_deployments_ns ON agent_deployments(namespace);

-- Durable build queue (workers claim with SKIP LOCKED)
CREATE TABLE build_jobs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    owner_id UUID NOT NULL,
    payload JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','in_progress','done','failed')),
    attempt INTEGER NOT NULL DEFAULT 0,
    error_msg TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    picked_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ
);
CREATE INDEX build_jobs_work_queue ON build_jobs(status, created_at) WHERE status IN ('pending', 'in_progress');

-- Upload pipeline state machine
CREATE TABLE upload_status (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    upload_id VARCHAR(255) NOT NULL UNIQUE,
    agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    agent_name VARCHAR(255) NOT NULL,
    status upload_pipeline_status NOT NULL DEFAULT 'initiated',
    owner_id UUID REFERENCES users(id) ON DELETE SET NULL,
    error_message TEXT,
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_upload_status_agent ON upload_status(agent_id);
CREATE INDEX idx_upload_status_inflight ON upload_status(status) WHERE status NOT IN ('completed', 'failed');
CREATE INDEX idx_upload_status_owner ON upload_status(owner_id, created_at DESC);
CREATE INDEX idx_upload_status_name ON upload_status(agent_name, created_at DESC);
CREATE TRIGGER trg_upload_status_updated_at BEFORE UPDATE ON upload_status FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- =============================================================================
-- Chat
-- =============================================================================

CREATE TABLE chat_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id TEXT NOT NULL UNIQUE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    agent_url TEXT,
    title TEXT NOT NULL,
    deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_sessions_user ON chat_sessions(user_id);
CREATE INDEX idx_sessions_created ON chat_sessions(created_at DESC);
CREATE INDEX idx_chat_sessions_recent ON chat_sessions(user_id, created_at DESC);
CREATE TRIGGER trg_chat_sessions_updated_at BEFORE UPDATE ON chat_sessions FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE chat_messages (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id TEXT NOT NULL REFERENCES chat_sessions(session_id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    file_parts JSONB,
    message_id VARCHAR(255),
    context_id VARCHAR(255),
    task_id VARCHAR(255),
    metadata JSONB NOT NULL DEFAULT '{}',
    has_file_parts BOOLEAN NOT NULL DEFAULT false,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_messages_session ON chat_messages(session_id, timestamp);

CREATE TABLE chat_message_files (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    message_id UUID,
    session_id TEXT NOT NULL,
    filename VARCHAR(500) NOT NULL,
    mime_type VARCHAR(255) NOT NULL,
    size_bytes BIGINT NOT NULL,
    storage_uri TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_chat_message_files_msg ON chat_message_files(message_id);
CREATE INDEX idx_chat_message_files_session ON chat_message_files(session_id);

-- =============================================================================
-- OCI Registry
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
-- Artifacts (OCI catalog with vector search)
-- =============================================================================

CREATE TABLE artifacts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner VARCHAR(255) NOT NULL,
    name VARCHAR(255) NOT NULL,
    version VARCHAR(50) NOT NULL,
    artifact_type VARCHAR(50) NOT NULL,
    status artifact_status NOT NULL DEFAULT 'preview',
    description TEXT,
    metadata JSONB NOT NULL DEFAULT '{}',
    oci_digest VARCHAR(71),
    size_bytes BIGINT,
    tags TEXT[] NOT NULL DEFAULT '{}',
    framework VARCHAR(50),
    license VARCHAR(50),
    embedding vector(1536),
    search_vector tsvector GENERATED ALWAYS AS (
        to_tsvector('english'::regconfig,
            coalesce(name, '') || ' ' || coalesce(description, '') || ' ' || text_array_to_string(tags, ' ')
        )
    ) STORED,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (owner, name, version)
);
CREATE INDEX idx_artifacts_type_status ON artifacts(artifact_type, status);
CREATE INDEX idx_artifacts_framework ON artifacts(artifact_type, framework, status, created_at DESC) WHERE status != 'yanked';
CREATE INDEX idx_artifacts_framework_created ON artifacts(framework, created_at DESC) WHERE status != 'yanked';
CREATE INDEX idx_artifacts_owner_name ON artifacts(owner, name);
CREATE INDEX idx_artifacts_tags ON artifacts USING gin(tags);
CREATE INDEX idx_artifacts_fts ON artifacts USING gin(search_vector);
CREATE INDEX idx_artifacts_embedding ON artifacts USING ivfflat (embedding vector_cosine_ops) WITH (lists = 100);
CREATE TRIGGER trg_artifacts_updated_at BEFORE UPDATE ON artifacts FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- =============================================================================
-- Observability & Audit
-- =============================================================================

CREATE TABLE proxy_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    caller_id UUID NOT NULL REFERENCES users(id),
    target_agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
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

-- Token usage tracking
CREATE TABLE token_usage (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    operation_type TEXT NOT NULL,
    request_id TEXT,
    session_id TEXT,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_input_tokens INTEGER DEFAULT 0,
    cache_read_input_tokens INTEGER DEFAULT 0,
    cached_tokens INTEGER DEFAULT 0,
    audio_tokens INTEGER DEFAULT 0,
    reasoning_tokens INTEGER DEFAULT 0,
    accepted_prediction_tokens INTEGER DEFAULT 0,
    rejected_prediction_tokens INTEGER DEFAULT 0,
    completion_tokens_details JSONB,
    prompt_tokens_details JSONB,
    cost_usd DECIMAL(10, 8),
    latency_ms INTEGER,
    ttft_ms INTEGER,
    streaming BOOLEAN DEFAULT false,
    finish_reason TEXT,
    metadata JSONB DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_token_usage_user_time ON token_usage(user_id, created_at DESC);
CREATE INDEX idx_token_usage_agent_time ON token_usage(agent_id, created_at DESC) WHERE agent_id IS NOT NULL;
CREATE INDEX idx_token_usage_operation ON token_usage(operation_type, created_at DESC);
CREATE INDEX idx_token_usage_provider ON token_usage(provider, created_at DESC);
CREATE INDEX idx_token_usage_model ON token_usage(model, created_at DESC);
CREATE INDEX idx_token_usage_session ON token_usage(session_id) WHERE session_id IS NOT NULL;
CREATE INDEX idx_token_usage_request ON token_usage(request_id) WHERE request_id IS NOT NULL;
CREATE INDEX idx_token_usage_cost ON token_usage(created_at DESC, cost_usd) WHERE cost_usd IS NOT NULL;
CREATE INDEX idx_token_usage_metadata ON token_usage USING gin(metadata);

-- Model pricing (for cost calculation)
CREATE TABLE model_pricing (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    input_price_per_1m DECIMAL(10, 4) NOT NULL,
    output_price_per_1m DECIMAL(10, 4) NOT NULL,
    cache_creation_price_per_1m DECIMAL(10, 4),
    cache_read_price_per_1m DECIMAL(10, 4),
    effective_from TIMESTAMPTZ NOT NULL DEFAULT now(),
    effective_until TIMESTAMPTZ,
    currency TEXT NOT NULL DEFAULT 'USD',
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(provider, model, effective_from)
);
CREATE INDEX idx_model_pricing_lookup ON model_pricing(provider, model, effective_from DESC);
CREATE TRIGGER trg_model_pricing_updated_at BEFORE UPDATE ON model_pricing FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Auto-calculate cost on insert
CREATE OR REPLACE FUNCTION calculate_token_cost(
    p_provider TEXT, p_model TEXT,
    p_input_tokens INTEGER, p_output_tokens INTEGER,
    p_cache_creation_tokens INTEGER, p_cache_read_tokens INTEGER,
    p_timestamp TIMESTAMPTZ
) RETURNS DECIMAL(10, 8) AS $$
DECLARE v_pricing RECORD; v_cost DECIMAL(10, 8);
BEGIN
    SELECT * INTO v_pricing FROM model_pricing
    WHERE provider = p_provider AND model = p_model
      AND effective_from <= p_timestamp
      AND (effective_until IS NULL OR effective_until > p_timestamp)
    ORDER BY effective_from DESC LIMIT 1;
    IF NOT FOUND THEN RETURN NULL; END IF;
    v_cost := (p_input_tokens::DECIMAL / 1000000.0) * v_pricing.input_price_per_1m
            + (p_output_tokens::DECIMAL / 1000000.0) * v_pricing.output_price_per_1m;
    IF v_pricing.cache_creation_price_per_1m IS NOT NULL THEN
        v_cost := v_cost
            + (COALESCE(p_cache_creation_tokens, 0)::DECIMAL / 1000000.0) * v_pricing.cache_creation_price_per_1m
            + (COALESCE(p_cache_read_tokens, 0)::DECIMAL / 1000000.0) * v_pricing.cache_read_price_per_1m;
    END IF;
    RETURN v_cost;
END;
$$ LANGUAGE plpgsql STABLE;

CREATE OR REPLACE FUNCTION calculate_usage_cost_trigger() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.cost_usd IS NULL THEN
        NEW.cost_usd := calculate_token_cost(
            NEW.provider, NEW.model, NEW.input_tokens, NEW.output_tokens,
            NEW.cache_creation_input_tokens, NEW.cache_read_input_tokens, NEW.created_at
        );
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trigger_calculate_usage_cost
    BEFORE INSERT ON token_usage FOR EACH ROW EXECUTE FUNCTION calculate_usage_cost_trigger();

-- Daily aggregates (materialized view)
CREATE MATERIALIZED VIEW token_usage_daily AS
SELECT user_id, agent_id, operation_type, provider, model,
    DATE(created_at) as date, COUNT(*) as request_count,
    SUM(input_tokens) as total_input_tokens, SUM(output_tokens) as total_output_tokens,
    SUM(total_tokens) as total_tokens, SUM(cache_creation_input_tokens) as total_cache_creation_tokens,
    SUM(cache_read_input_tokens) as total_cache_read_tokens, SUM(cached_tokens) as total_cached_tokens,
    SUM(reasoning_tokens) as total_reasoning_tokens, SUM(cost_usd) as total_cost_usd,
    AVG(latency_ms) as avg_latency_ms
FROM token_usage
GROUP BY user_id, agent_id, operation_type, provider, model, DATE(created_at);

CREATE UNIQUE INDEX idx_token_usage_daily_unique
    ON token_usage_daily(user_id, COALESCE(agent_id, '00000000-0000-0000-0000-000000000000'::uuid), operation_type, provider, model, date);

-- Router request logs
CREATE TABLE router_request_log (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    request_id TEXT NOT NULL UNIQUE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    query TEXT NOT NULL,
    agents_considered INTEGER NOT NULL DEFAULT 0,
    selected_agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    selected_agent_name TEXT,
    selection_reasoning TEXT,
    fallback_used BOOLEAN DEFAULT false,
    selection_token_usage_id UUID REFERENCES token_usage(id) ON DELETE SET NULL,
    total_latency_ms INTEGER NOT NULL,
    registry_fetch_ms INTEGER,
    vector_store_ms INTEGER,
    selection_llm_ms INTEGER,
    agent_call_ms INTEGER,
    success BOOLEAN NOT NULL,
    error_message TEXT,
    finish_reason TEXT,
    streaming BOOLEAN DEFAULT false,
    file_count INTEGER DEFAULT 0,
    metadata JSONB DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_router_log_user_time ON router_request_log(user_id, created_at DESC);
CREATE INDEX idx_router_log_session ON router_request_log(session_id, created_at DESC);
CREATE INDEX idx_router_log_agent ON router_request_log(selected_agent_id, created_at DESC) WHERE selected_agent_id IS NOT NULL;
CREATE INDEX idx_router_log_success ON router_request_log(success, created_at DESC);

-- Agent selection analytics (materialized view)
CREATE MATERIALIZED VIEW agent_selection_stats AS
SELECT selected_agent_id, selected_agent_name, COUNT(*) as selection_count,
    COUNT(*) FILTER (WHERE success = true) as successful_calls,
    COUNT(*) FILTER (WHERE success = false) as failed_calls,
    AVG(agent_call_ms) FILTER (WHERE agent_call_ms IS NOT NULL) as avg_agent_latency_ms,
    AVG(selection_llm_ms) FILTER (WHERE selection_llm_ms IS NOT NULL) as avg_selection_latency_ms,
    DATE(created_at) as date
FROM router_request_log WHERE selected_agent_id IS NOT NULL
GROUP BY selected_agent_id, selected_agent_name, DATE(created_at);

CREATE UNIQUE INDEX idx_agent_selection_stats_unique ON agent_selection_stats(selected_agent_id, date);

-- =============================================================================
-- Multi-Agent Flows
-- =============================================================================

CREATE TABLE flows (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    flow_id TEXT NOT NULL UNIQUE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    root_agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    root_agent_name TEXT,
    title TEXT,
    status TEXT NOT NULL DEFAULT 'running',
    max_depth_reached INTEGER NOT NULL DEFAULT 0,
    total_invocations INTEGER NOT NULL DEFAULT 0,
    total_tokens_used BIGINT NOT NULL DEFAULT 0,
    total_cost_usd DECIMAL(10, 8),
    duration_ms BIGINT,
    error_message TEXT,
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);
CREATE INDEX idx_flows_user_time ON flows(user_id, created_at DESC);
CREATE INDEX idx_flows_status ON flows(status);
CREATE INDEX idx_flows_root_agent ON flows(root_agent_id) WHERE root_agent_id IS NOT NULL;

CREATE TABLE flow_steps (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    flow_id TEXT NOT NULL REFERENCES flows(flow_id) ON DELETE CASCADE,
    step_order INTEGER NOT NULL,
    depth INTEGER NOT NULL DEFAULT 0,
    agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    agent_name TEXT NOT NULL,
    caller_agent_name TEXT,
    input_summary TEXT,
    output_summary TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    tokens_used INTEGER NOT NULL DEFAULT 0,
    latency_ms INTEGER,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);
CREATE INDEX idx_flow_steps_flow ON flow_steps(flow_id, step_order);
CREATE INDEX idx_flow_steps_agent ON flow_steps(agent_id) WHERE agent_id IS NOT NULL;

-- =============================================================================
-- Seed: model pricing
-- =============================================================================

INSERT INTO model_pricing (provider, model, input_price_per_1m, output_price_per_1m, cache_creation_price_per_1m, cache_read_price_per_1m, notes) VALUES
('openai', 'gpt-4o', 2.50, 10.00, NULL, NULL, 'GPT-4o standard'),
('openai', 'gpt-4o-mini', 0.15, 0.60, NULL, NULL, 'GPT-4o mini'),
('openai', 'o1-preview', 15.00, 60.00, NULL, NULL, 'o1 preview'),
('openai', 'o1-mini', 3.00, 12.00, NULL, NULL, 'o1 mini'),
('anthropic', 'claude-opus-4', 15.00, 75.00, 18.75, 1.50, 'Claude Opus 4'),
('anthropic', 'claude-sonnet-4', 3.00, 15.00, 3.75, 0.30, 'Claude Sonnet 4'),
('anthropic', 'claude-haiku-4', 0.80, 4.00, 1.00, 0.08, 'Claude Haiku 4'),
('groq', 'llama-3.3-70b-versatile', 0.59, 0.79, NULL, NULL, 'Llama 3.3 70B on Groq'),
('groq', 'llama-3.1-8b-instant', 0.05, 0.08, NULL, NULL, 'Llama 3.1 8B on Groq'),
('deepseek', 'deepseek-chat', 0.14, 0.28, 0.014, 0.014, 'DeepSeek Chat'),
('deepseek', 'deepseek-reasoner', 0.55, 2.19, NULL, NULL, 'DeepSeek R1');
