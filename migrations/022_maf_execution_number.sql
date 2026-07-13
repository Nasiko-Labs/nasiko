-- =============================================================================
-- Migration 022: user-facing incremental execution number
--
-- maf_executions.id (UUID) stays the internal identifier — it's the A2A
-- contextId threading all agent calls in a run, and the Redis job/retry key.
-- execution_number is a purely cosmetic, globally incrementing display id
-- (Postgres IDENTITY sequence — concurrency-safe with no app-level locking).
-- =============================================================================

ALTER TABLE maf_executions
    ADD COLUMN execution_number BIGINT GENERATED ALWAYS AS IDENTITY;

CREATE UNIQUE INDEX idx_maf_executions_number ON maf_executions (execution_number);
