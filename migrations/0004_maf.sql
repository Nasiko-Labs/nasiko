-- =============================================================================
-- Multi-Agent Flow (MAF) tables
--
-- mafs              — workflow definitions (soft-delete via status column)
-- maf_executions    — per-run state with retry counters
-- Indexes for fast list-my-MAFs, ownership checks, and history queries
-- =============================================================================

-- =============================================================================
-- Table: mafs
-- Stores saved workflow definitions.  Soft-delete via status = 'deleted'.
-- Rows are never hard-deleted; maf_id is retained in maf_executions history.
-- =============================================================================

CREATE TABLE mafs (
    id          UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID         NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        TEXT         NOT NULL,
    description TEXT,
    maf_json    JSONB        NOT NULL,
    status      VARCHAR(255) NOT NULL DEFAULT 'active',
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT now()
);

-- Partial index skips deleted rows in list-my-MAFs queries
CREATE INDEX idx_mafs_user ON mafs (user_id) WHERE status = 'active';

-- =============================================================================
-- Table: maf_executions
-- One row per execution run.  Status written to DB on every transition.
-- DB is the single source of truth — no Redis status cache.
-- The execution id doubles as the A2A context_id so all steps share one thread.
--
-- execution_number is a purely cosmetic, globally incrementing display id
-- (Postgres IDENTITY sequence — concurrency-safe with no app-level locking);
-- id (UUID) stays the internal identifier.
-- =============================================================================

CREATE TABLE maf_executions (
    id            UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    maf_id        UUID         REFERENCES mafs(id) ON DELETE SET NULL,
    user_id       UUID         NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status        VARCHAR(255) NOT NULL DEFAULT 'pending',
    attempt_count INTEGER      NOT NULL DEFAULT 0,
    max_attempts  INTEGER      NOT NULL DEFAULT 3,
    tokens_used   BIGINT       NOT NULL DEFAULT 0,
    started_at    TIMESTAMPTZ,
    completed_at  TIMESTAMPTZ,
    duration_ms   BIGINT,
    output        TEXT,
    step_results  JSONB,
    error         TEXT,
    created_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    execution_number BIGINT GENERATED ALWAYS AS IDENTITY
);

CREATE INDEX idx_maf_executions_user ON maf_executions (user_id);
CREATE INDEX idx_maf_executions_maf  ON maf_executions (maf_id);
CREATE UNIQUE INDEX idx_maf_executions_number ON maf_executions (execution_number);
