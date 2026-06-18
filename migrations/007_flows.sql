-- =============================================================================
-- Multi-Agent Flows
-- Persistent record of multi-agent workflow executions
-- =============================================================================

CREATE TABLE flows (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    flow_id TEXT NOT NULL UNIQUE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    root_agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    root_agent_name TEXT,
    title TEXT,
    status TEXT NOT NULL DEFAULT 'running',  -- 'running', 'completed', 'failed', 'timeout'
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
CREATE INDEX idx_flows_flow_id ON flows(flow_id);

-- Flow steps: each agent invocation within a flow
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
    status TEXT NOT NULL DEFAULT 'pending',  -- 'pending', 'running', 'completed', 'failed'
    tokens_used INTEGER NOT NULL DEFAULT 0,
    latency_ms INTEGER,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX idx_flow_steps_flow ON flow_steps(flow_id, step_order);
CREATE INDEX idx_flow_steps_agent ON flow_steps(agent_id) WHERE agent_id IS NOT NULL;
