-- Admin-configurable agent-catalog tab list (comma-separated tag names).
-- NULL/empty means the UI derives tabs from the most common agent tags.
ALTER TABLE settings ADD COLUMN catalog_tabs TEXT;
