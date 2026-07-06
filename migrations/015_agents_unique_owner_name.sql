-- SRV-2: enforce one active agent name per owner.
--
-- Without this, the upload path's SELECT-then-INSERT upsert had a TOCTOU race
-- (two concurrent same-name uploads both miss the SELECT and both INSERT, leaving
-- duplicate rows that later reads pick from arbitrarily), and the catalog create
-- handler's 23505 -> 409 mapping was dead code that could never fire.
--
-- Partial index (active rows only) so a soft-deleted agent's name can be reused.
-- Prerequisite: no existing duplicate (owner_id, name) among non-deleted rows —
-- true for fresh installs; a dirty DB must dedupe before this migration runs.
CREATE UNIQUE INDEX IF NOT EXISTS agents_owner_name_active_uniq
    ON agents (owner_id, name)
    WHERE deleted_at IS NULL;
