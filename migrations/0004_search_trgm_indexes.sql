-- Migration 013: trigram indexes for search
--
-- Queries use col ILIKE $1 (see catalog/routes.rs search / search_users).
-- pg_trgm GIN indexes only activate when the indexed expression matches the
-- query expression exactly — lower(col) defeats them.
--
-- agents.name already has idx_agents_name_trgm (001_schema).
-- Add the two remaining searched agent columns and all three user columns.
CREATE INDEX IF NOT EXISTS idx_agents_display_name_trgm
    ON agents USING gin(display_name gin_trgm_ops);

CREATE INDEX IF NOT EXISTS idx_agents_description_trgm
    ON agents USING gin(description gin_trgm_ops);

CREATE INDEX IF NOT EXISTS idx_users_username_trgm
    ON users USING gin(username gin_trgm_ops);

CREATE INDEX IF NOT EXISTS idx_users_display_name_trgm
    ON users USING gin(display_name gin_trgm_ops);

CREATE INDEX IF NOT EXISTS idx_users_email_trgm
    ON users USING gin(email gin_trgm_ops);
