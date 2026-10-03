-- =============================================================================
-- Draft origin
--
-- `mafs.status` records what a workflow IS right now ('draft' | 'active' |
-- 'deleted'). It cannot also record where it came from: promoting a draft
-- overwrites 'draft' with 'active', and the fact that the row was ever drafted
-- is gone.
--
-- `drafted_at` records that origin once and is never cleared. It lets the
-- drafts list keep showing a workflow after it has been deployed, so its owner
-- can watch what they drafted accumulate runs and spend rather than having it
-- vanish from the list the moment it goes live.
--
-- Nullable with no default on purpose: NULL means "never was a draft", which is
-- the correct answer for every workflow created directly.
-- =============================================================================

ALTER TABLE mafs ADD COLUMN drafted_at TIMESTAMPTZ;

-- Backfill: rows sitting at 'draft' today were drafted when they were created.
UPDATE mafs SET drafted_at = created_at WHERE status = 'draft';

-- Partial index matching the drafts list predicate exactly, so that listing
-- never scans workflows that were never drafted.
CREATE INDEX idx_mafs_drafted ON mafs (user_id, drafted_at DESC)
    WHERE drafted_at IS NOT NULL;
