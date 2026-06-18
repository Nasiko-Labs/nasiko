-- =============================================================================
-- Router Request Logs
-- Track every routing decision with full context
-- =============================================================================

CREATE TABLE router_request_log (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Request Context
    request_id TEXT NOT NULL UNIQUE,  -- External correlation ID
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    query TEXT NOT NULL,

    -- Agent Selection
    agents_considered INTEGER NOT NULL DEFAULT 0,
    selected_agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    selected_agent_name TEXT,
    selection_reasoning TEXT,  -- Why this agent was chosen
    fallback_used BOOLEAN DEFAULT false,

    -- Token Usage (links to token_usage table)
    selection_token_usage_id UUID REFERENCES token_usage(id) ON DELETE SET NULL,

    -- Performance Metrics (milliseconds)
    total_latency_ms INTEGER NOT NULL,
    registry_fetch_ms INTEGER,  -- Time to fetch agent list
    vector_store_ms INTEGER,    -- Time to build vector store
    selection_llm_ms INTEGER,   -- Time for LLM to select agent
    agent_call_ms INTEGER,      -- Time for agent to respond

    -- Outcome
    success BOOLEAN NOT NULL,
    error_message TEXT,
    finish_reason TEXT,  -- 'success', 'no_agents', 'agent_error', 'timeout', 'llm_error'

    -- Request Metadata
    streaming BOOLEAN DEFAULT false,
    file_count INTEGER DEFAULT 0,
    metadata JSONB DEFAULT '{}',

    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Indexes for common queries
CREATE INDEX idx_router_log_user_time ON router_request_log(user_id, created_at DESC);
CREATE INDEX idx_router_log_session ON router_request_log(session_id, created_at DESC);
CREATE INDEX idx_router_log_agent ON router_request_log(selected_agent_id, created_at DESC) WHERE selected_agent_id IS NOT NULL;
CREATE INDEX idx_router_log_request ON router_request_log(request_id);
CREATE INDEX idx_router_log_success ON router_request_log(success, created_at DESC);
CREATE INDEX idx_router_log_created ON router_request_log(created_at DESC);

-- =============================================================================
-- Agent Selection Analytics (Materialized View)
-- =============================================================================

CREATE MATERIALIZED VIEW agent_selection_stats AS
SELECT
    selected_agent_id,
    selected_agent_name,
    COUNT(*) as selection_count,
    COUNT(*) FILTER (WHERE success = true) as successful_calls,
    COUNT(*) FILTER (WHERE success = false) as failed_calls,
    AVG(agent_call_ms) FILTER (WHERE agent_call_ms IS NOT NULL) as avg_agent_latency_ms,
    AVG(selection_llm_ms) FILTER (WHERE selection_llm_ms IS NOT NULL) as avg_selection_latency_ms,
    DATE(created_at) as date
FROM router_request_log
WHERE selected_agent_id IS NOT NULL
GROUP BY selected_agent_id, selected_agent_name, DATE(created_at);

CREATE UNIQUE INDEX idx_agent_selection_stats_unique
    ON agent_selection_stats(selected_agent_id, date);

-- Refresh function
CREATE OR REPLACE FUNCTION refresh_agent_selection_stats()
RETURNS void AS $$
BEGIN
    REFRESH MATERIALIZED VIEW CONCURRENTLY agent_selection_stats;
END;
$$ LANGUAGE plpgsql;
