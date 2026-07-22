-- Per-user "pinned" connectors, for a quick-access shortlist in the catalog UI.
-- "Recent" is intentionally NOT a new table here — it's derived from
-- mcp_user_connections.updated_at (real connect/reconnect activity), which
-- already exists and avoids a write on every catalog page view.
CREATE TABLE mcp_connector_pins (
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    connector_id UUID NOT NULL REFERENCES mcp_connectors(id) ON DELETE CASCADE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, connector_id)
);
CREATE INDEX idx_mcp_connector_pins_user ON mcp_connector_pins(user_id, created_at DESC);
