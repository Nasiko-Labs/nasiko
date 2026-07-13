-- =============================================================================
-- `oss/server/src/settings.rs` has always read/written a `settings` table that
-- no prior migration ever created — GET/PUT /api/settings 500s unconditionally
-- on any database. Singleton row (id = 1), matching the handler's
-- `INSERT ... VALUES (1, ...) ON CONFLICT (id) DO UPDATE` pattern.
-- =============================================================================

CREATE TABLE settings (
    id               INT PRIMARY KEY,
    router_model     TEXT,
    default_provider TEXT,
    max_flow_depth   INT,
    max_flow_fan_out INT,
    max_flow_tokens  BIGINT,
    flow_timeout_secs INT,
    registry_url     TEXT
);
