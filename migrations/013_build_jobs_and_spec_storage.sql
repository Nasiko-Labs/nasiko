-- Store deploy-time spec so restart can reconstruct it without hardcoding defaults.
ALTER TABLE agent_deployments
    ADD COLUMN IF NOT EXISTS spec_ports  INTEGER[]  DEFAULT '{8000}',
    ADD COLUMN IF NOT EXISTS spec_image  TEXT;

-- Postgres-backed durable build queue: workers claim jobs with SKIP LOCKED.
CREATE TABLE IF NOT EXISTS build_jobs (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id     UUID        NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    owner_id     UUID        NOT NULL,
    payload      JSONB       NOT NULL,   -- serialized DeploymentSpec + build context path
    status       TEXT        NOT NULL DEFAULT 'pending'
                             CHECK (status IN ('pending','in_progress','done','failed')),
    attempt      INTEGER     NOT NULL DEFAULT 0,
    error_msg    TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    picked_at    TIMESTAMPTZ,
    completed_at TIMESTAMPTZ
);

-- Partial index: only the rows workers query — pending first, then long-running in_progress.
CREATE INDEX IF NOT EXISTS build_jobs_work_queue
    ON build_jobs (status, created_at)
    WHERE status IN ('pending', 'in_progress');
