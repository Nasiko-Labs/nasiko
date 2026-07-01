-- =============================================================================
-- Router v2: Stage tracking columns + refreshed analytics view
-- =============================================================================

-- Add Stage 1 / Stage 2 pipeline columns.
-- fallback_used, idx_router_log_session, idx_router_log_agent already exist
-- from 003_router_logs.sql — only add what is new.

ALTER TABLE router_request_log
    ADD COLUMN IF NOT EXISTS stage1_candidates INT,
    ADD COLUMN IF NOT EXISTS stage2_candidates INT,
    ADD COLUMN IF NOT EXISTS embedding_model   TEXT;

-- =============================================================================
-- Refresh agent_selection_stats to include stage averages
-- Must DROP first because materialized views cannot be altered in-place.
-- =============================================================================

DROP MATERIALIZED VIEW IF EXISTS agent_selection_stats;

CREATE MATERIALIZED VIEW agent_selection_stats AS
SELECT
    selected_agent_id,
    selected_agent_name,
    COUNT(*)                                                          AS selection_count,
    COUNT(*) FILTER (WHERE success = true)                            AS successful_calls,
    COUNT(*) FILTER (WHERE success = false)                           AS failed_calls,
    AVG(agent_call_ms)       FILTER (WHERE agent_call_ms IS NOT NULL) AS avg_agent_latency_ms,
    AVG(selection_llm_ms)    FILTER (WHERE selection_llm_ms IS NOT NULL) AS avg_selection_latency_ms,
    AVG(stage1_candidates)   FILTER (WHERE stage1_candidates IS NOT NULL) AS avg_stage1_candidates,
    AVG(stage2_candidates)   FILTER (WHERE stage2_candidates IS NOT NULL) AS avg_stage2_candidates,
    DATE(created_at)                                                  AS date
FROM router_request_log
WHERE selected_agent_id IS NOT NULL
GROUP BY selected_agent_id, selected_agent_name, DATE(created_at);

CREATE UNIQUE INDEX idx_agent_selection_stats_unique
    ON agent_selection_stats(selected_agent_id, date);

-- Refresh function (unchanged signature, recreated because view was dropped)
CREATE OR REPLACE FUNCTION refresh_agent_selection_stats()
RETURNS void AS $$
BEGIN
    REFRESH MATERIALIZED VIEW CONCURRENTLY agent_selection_stats;
END;
$$ LANGUAGE plpgsql;
