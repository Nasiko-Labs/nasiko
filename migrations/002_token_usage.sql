-- =============================================================================
-- Token Usage Tracking
-- Comprehensive tracking for all LLM usage across the platform
-- =============================================================================

CREATE TABLE token_usage (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Context
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    operation_type TEXT NOT NULL,  -- 'router_selection', 'agent_call', 'capability_generation', 'chat', 'direct_llm'
    request_id TEXT,  -- External request ID for correlation
    session_id TEXT,  -- Chat session ID if applicable

    -- Provider & Model
    provider TEXT NOT NULL,  -- 'openai', 'anthropic', 'groq', 'deepseek', etc.
    model TEXT NOT NULL,  -- 'gpt-4o', 'claude-sonnet-4', 'llama-3.3-70b', etc.

    -- Core Token Counts (always present)
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,

    -- Prompt Caching (Anthropic-style)
    cache_creation_input_tokens INTEGER DEFAULT 0,  -- Tokens written to cache
    cache_read_input_tokens INTEGER DEFAULT 0,      -- Tokens read from cache

    -- OpenAI Prompt Details
    cached_tokens INTEGER DEFAULT 0,                -- OpenAI: tokens served from cache
    audio_tokens INTEGER DEFAULT 0,                 -- OpenAI: audio input tokens

    -- OpenAI Completion Details
    reasoning_tokens INTEGER DEFAULT 0,             -- OpenAI o1: reasoning tokens
    accepted_prediction_tokens INTEGER DEFAULT 0,   -- OpenAI: predicted outputs accepted
    rejected_prediction_tokens INTEGER DEFAULT 0,   -- OpenAI: predicted outputs rejected

    -- Additional Metrics
    completion_tokens_details JSONB,  -- Full completion_tokens_details from provider
    prompt_tokens_details JSONB,      -- Full prompt_tokens_details from provider

    -- Cost Calculation
    cost_usd DECIMAL(10, 8),  -- Calculated cost in USD (8 decimals for precision)

    -- Performance Metrics
    latency_ms INTEGER,  -- Time to first token or completion
    ttft_ms INTEGER,     -- Time to first token (streaming)

    -- Request Metadata
    streaming BOOLEAN DEFAULT false,
    finish_reason TEXT,  -- 'stop', 'length', 'content_filter', 'tool_calls', etc.
    metadata JSONB DEFAULT '{}',  -- Additional provider-specific data

    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Indexes for common queries
CREATE INDEX idx_token_usage_user_time ON token_usage(user_id, created_at DESC);
CREATE INDEX idx_token_usage_agent_time ON token_usage(agent_id, created_at DESC) WHERE agent_id IS NOT NULL;
CREATE INDEX idx_token_usage_operation ON token_usage(operation_type, created_at DESC);
CREATE INDEX idx_token_usage_provider ON token_usage(provider, created_at DESC);
CREATE INDEX idx_token_usage_model ON token_usage(model, created_at DESC);
CREATE INDEX idx_token_usage_session ON token_usage(session_id) WHERE session_id IS NOT NULL;
CREATE INDEX idx_token_usage_request ON token_usage(request_id) WHERE request_id IS NOT NULL;

-- Partial index for cost analysis (only rows with cost)
CREATE INDEX idx_token_usage_cost ON token_usage(created_at DESC, cost_usd) WHERE cost_usd IS NOT NULL;

-- GIN index for JSONB metadata queries
CREATE INDEX idx_token_usage_metadata ON token_usage USING gin(metadata);

-- =============================================================================
-- Token Usage Aggregates (Materialized View for Dashboard)
-- =============================================================================

CREATE MATERIALIZED VIEW token_usage_daily AS
SELECT
    user_id,
    agent_id,
    operation_type,
    provider,
    model,
    DATE(created_at) as date,
    COUNT(*) as request_count,
    SUM(input_tokens) as total_input_tokens,
    SUM(output_tokens) as total_output_tokens,
    SUM(total_tokens) as total_tokens,
    SUM(cache_creation_input_tokens) as total_cache_creation_tokens,
    SUM(cache_read_input_tokens) as total_cache_read_tokens,
    SUM(cached_tokens) as total_cached_tokens,
    SUM(reasoning_tokens) as total_reasoning_tokens,
    SUM(cost_usd) as total_cost_usd,
    AVG(latency_ms) as avg_latency_ms
FROM token_usage
GROUP BY user_id, agent_id, operation_type, provider, model, DATE(created_at);

CREATE UNIQUE INDEX idx_token_usage_daily_unique
    ON token_usage_daily(user_id, COALESCE(agent_id, '00000000-0000-0000-0000-000000000000'::uuid), operation_type, provider, model, date);

-- Refresh function (call daily via cron or trigger)
CREATE OR REPLACE FUNCTION refresh_token_usage_daily()
RETURNS void AS $$
BEGIN
    REFRESH MATERIALIZED VIEW CONCURRENTLY token_usage_daily;
END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- Model Pricing (for cost calculation)
-- =============================================================================

CREATE TABLE model_pricing (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider TEXT NOT NULL,
    model TEXT NOT NULL,

    -- Pricing per 1M tokens (easier to work with than per token)
    input_price_per_1m DECIMAL(10, 4) NOT NULL,
    output_price_per_1m DECIMAL(10, 4) NOT NULL,

    -- Prompt caching pricing (if supported)
    cache_creation_price_per_1m DECIMAL(10, 4),
    cache_read_price_per_1m DECIMAL(10, 4),

    -- Effective date range
    effective_from TIMESTAMPTZ NOT NULL DEFAULT now(),
    effective_until TIMESTAMPTZ,

    -- Metadata
    currency TEXT NOT NULL DEFAULT 'USD',
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE(provider, model, effective_from)
);

CREATE INDEX idx_model_pricing_lookup ON model_pricing(provider, model, effective_from DESC);

-- Insert current pricing (as of 2024)
INSERT INTO model_pricing (provider, model, input_price_per_1m, output_price_per_1m, cache_creation_price_per_1m, cache_read_price_per_1m, notes) VALUES
-- OpenAI
('openai', 'gpt-4o', 2.50, 10.00, NULL, NULL, 'GPT-4o standard'),
('openai', 'gpt-4o-mini', 0.15, 0.60, NULL, NULL, 'GPT-4o mini'),
('openai', 'gpt-4-turbo', 10.00, 30.00, NULL, NULL, 'GPT-4 Turbo'),
('openai', 'gpt-3.5-turbo', 0.50, 1.50, NULL, NULL, 'GPT-3.5 Turbo'),
('openai', 'o1-preview', 15.00, 60.00, NULL, NULL, 'o1 preview (reasoning)'),
('openai', 'o1-mini', 3.00, 12.00, NULL, NULL, 'o1 mini (reasoning)'),

-- Anthropic (with prompt caching)
('anthropic', 'claude-opus-4', 15.00, 75.00, 18.75, 1.50, 'Claude 4 Opus with prompt caching'),
('anthropic', 'claude-sonnet-4', 3.00, 15.00, 3.75, 0.30, 'Claude 4 Sonnet with prompt caching'),
('anthropic', 'claude-haiku-4', 0.80, 4.00, 1.00, 0.08, 'Claude 4 Haiku with prompt caching'),

-- Groq (very cheap, fast inference)
('groq', 'llama-3.3-70b-versatile', 0.59, 0.79, NULL, NULL, 'Llama 3.3 70B on Groq'),
('groq', 'llama-3.1-8b-instant', 0.05, 0.08, NULL, NULL, 'Llama 3.1 8B on Groq'),

-- DeepSeek
('deepseek', 'deepseek-chat', 0.14, 0.28, 0.014, 0.014, 'DeepSeek Chat with caching'),
('deepseek', 'deepseek-reasoner', 0.55, 2.19, NULL, NULL, 'DeepSeek R1 (reasoning)');

-- =============================================================================
-- Helper Functions
-- =============================================================================

-- Calculate cost for a usage record
CREATE OR REPLACE FUNCTION calculate_token_cost(
    p_provider TEXT,
    p_model TEXT,
    p_input_tokens INTEGER,
    p_output_tokens INTEGER,
    p_cache_creation_tokens INTEGER,
    p_cache_read_tokens INTEGER,
    p_timestamp TIMESTAMPTZ
) RETURNS DECIMAL(10, 8) AS $$
DECLARE
    v_pricing RECORD;
    v_cost DECIMAL(10, 8);
BEGIN
    -- Get pricing for the model at the given timestamp
    SELECT * INTO v_pricing
    FROM model_pricing
    WHERE provider = p_provider
      AND model = p_model
      AND effective_from <= p_timestamp
      AND (effective_until IS NULL OR effective_until > p_timestamp)
    ORDER BY effective_from DESC
    LIMIT 1;

    IF NOT FOUND THEN
        RETURN NULL;  -- No pricing data
    END IF;

    -- Calculate base cost
    v_cost := (p_input_tokens::DECIMAL / 1000000.0) * v_pricing.input_price_per_1m
            + (p_output_tokens::DECIMAL / 1000000.0) * v_pricing.output_price_per_1m;

    -- Add prompt caching costs if applicable
    IF v_pricing.cache_creation_price_per_1m IS NOT NULL THEN
        v_cost := v_cost
                + (COALESCE(p_cache_creation_tokens, 0)::DECIMAL / 1000000.0) * v_pricing.cache_creation_price_per_1m
                + (COALESCE(p_cache_read_tokens, 0)::DECIMAL / 1000000.0) * v_pricing.cache_read_price_per_1m;
    END IF;

    RETURN v_cost;
END;
$$ LANGUAGE plpgsql STABLE;

-- Trigger to auto-calculate cost on insert
CREATE OR REPLACE FUNCTION calculate_usage_cost_trigger()
RETURNS TRIGGER AS $$
BEGIN
    IF NEW.cost_usd IS NULL THEN
        NEW.cost_usd := calculate_token_cost(
            NEW.provider,
            NEW.model,
            NEW.input_tokens,
            NEW.output_tokens,
            NEW.cache_creation_input_tokens,
            NEW.cache_read_input_tokens,
            NEW.created_at
        );
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trigger_calculate_usage_cost
    BEFORE INSERT ON token_usage
    FOR EACH ROW
    EXECUTE FUNCTION calculate_usage_cost_trigger();
