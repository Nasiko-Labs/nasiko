-- =============================================================================
-- Public workflows and seeded workflow templates
--
-- `is_public` lets every user see and run a workflow they do not own, the way
-- `agents.is_public` does for agents. Editing and deleting stay with the
-- owner. Each run is still recorded under the user who started it, so users
-- of a public workflow see only their own runs.
--
-- `seed_template` and `seed_version` mark a workflow the server installed at
-- boot from `SEED_WORKFLOWS` (`oss/server/src/seed_workflows.rs`). The unique
-- index is what makes that install idempotent: a restart finds the template's
-- row and leaves it alone, and a higher version updates the same row in place,
-- so its id and run history survive the upgrade. NULL means "not seeded",
-- which is every workflow a user creates.
--
-- Numbered 0107, not 0105: `0105` and `0106` are taken on an unmerged branch.
-- =============================================================================

ALTER TABLE mafs
    ADD COLUMN is_public     BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN seed_template TEXT,
    ADD COLUMN seed_version  INTEGER;

CREATE UNIQUE INDEX mafs_seed_template ON mafs (seed_template)
    WHERE seed_template IS NOT NULL;

-- The workflow list unions the caller's own rows with the public ones; this
-- keeps the public half from scanning every workflow.
CREATE INDEX idx_mafs_public ON mafs (created_at)
    WHERE is_public AND status = 'active';
