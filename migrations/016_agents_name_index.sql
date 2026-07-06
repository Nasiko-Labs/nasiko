-- A-2 (OCI cross-tenant access control): the OCI registry's repo path is
-- name-only (the "owner" path segment carries no real per-tenant meaning —
-- every CLI caller sends a constant), so `check_repo_access` looks up an
-- agent by name alone, without an owner_id predicate. Migration 015's unique
-- index is (owner_id, name) — leftmost column owner_id — so it cannot serve
-- a name-only equality lookup; every OCI blob/manifest/tag request would
-- otherwise seq-scan `agents`, and a multi-layer image pull issues N scans.
--
-- The existing GIN trigram index (001_schema.sql, idx_agents_name_trgm) is
-- for ILIKE/similarity search, not plain equality — it won't be picked for
-- this predicate.
CREATE INDEX IF NOT EXISTS idx_agents_name_active
    ON agents (name)
    WHERE deleted_at IS NULL;
