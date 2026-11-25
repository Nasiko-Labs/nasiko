-- Replay evals the server runs (`/api/evals/*`).
--
-- A dataset is built from coding sessions a person picked: each session is linked to the commit
-- it produced (the repository comes from the session, 0116), split into the real fix and its
-- tests, and becomes a task. A task is rewritten as an issue, built and tested (its
-- FAIL_TO_PASS / PASS_TO_PASS lists), and screened by reviewer models; a verified task can be
-- run. A run replays chosen tasks on chosen models (each one of the person's own LLM configs)
-- and grades every attempt; its report is kept, so a past run reads the same forever.
--
-- Every row belongs to one user (through its dataset or run); the API never shows another
-- user's. The work itself is a queue of jobs (`eval_jobs`), claimed with SKIP LOCKED by any
-- server and kept alive by a heartbeat, so a long build or attempt survives a restart of
-- another replica and is taken over when its own replica dies.
--
-- cold: compatible (none of these tables is tiered)

CREATE TABLE eval_datasets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- The coding agent the sessions came from (`agents.coding_agent_integration_id`).
    harness TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'harvesting'
        CHECK (status IN ('harvesting', 'preparing', 'ready', 'failed')),
    -- `chat_sessions.session_id` of the picked sessions.
    session_ids TEXT[] NOT NULL,
    -- `llm_configs.id` of the model that rewrites requests as issues (prompt P1), and of the
    -- models that screen tasks (prompt P3); NULL / empty = the person reviews.
    rewriter_config_id UUID,
    reviewer_config_ids UUID[] NOT NULL DEFAULT '{}',
    -- Runs of each gate set-up, to catch flaky tests.
    gate_runs INT NOT NULL DEFAULT 3 CHECK (gate_runs BETWEEN 1 AND 5),
    -- What happened to each picked session: `{"<session_id>": "task" | "no_match" | ...}`.
    outcomes JSONB NOT NULL DEFAULT '{}',
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_eval_datasets_user ON eval_datasets (user_id, created_at DESC);

CREATE TABLE eval_tasks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    dataset_id UUID NOT NULL REFERENCES eval_datasets(id) ON DELETE CASCADE,
    -- `owner/name` on GitHub.
    repo TEXT NOT NULL,
    source_session_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'candidate'
        CHECK (status IN ('candidate', 'reproducible', 'verified', 'rejected')),
    -- The next step: rewrite (P1), build (image + gate), screen (P3), review (a person), done.
    stage TEXT NOT NULL DEFAULT 'rewrite'
        CHECK (stage IN ('rewrite', 'build', 'screen', 'review', 'done')),
    -- The SWE-bench row (`nasiko_evals::TaskInstance`).
    instance JSONB NOT NULL,
    -- What is kept of the session (`TaskSource`): typed messages around the task, usage.
    source JSONB NOT NULL,
    -- The link decision and every candidate commit's score.
    link JSONB NOT NULL,
    -- The gate's verdict and a short log per run.
    gate JSONB,
    -- Screen verdicts (`ScreenVerdict`), a person's last.
    reviews JSONB NOT NULL DEFAULT '[]',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_eval_tasks_dataset ON eval_tasks (dataset_id);

-- Build and test settings per user and repository (`nasiko_evals::spec::EnvSpec`), proposed
-- from the repository on first use and editable.
CREATE TABLE eval_specs (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    repo TEXT NOT NULL,
    spec JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, repo)
);

CREATE TABLE eval_runs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    dataset_id UUID NOT NULL REFERENCES eval_datasets(id) ON DELETE CASCADE,
    -- `[{id, name, provider, model, builder}]`: the LLM configs tested, as they were at start.
    models JSONB NOT NULL,
    attempts INT NOT NULL DEFAULT 1 CHECK (attempts BETWEEN 1 AND 5),
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued', 'running', 'finalizing', 'done', 'failed', 'cancelled')),
    -- The report (`ModelSummary[]` and the headline), written once every attempt ended.
    report JSONB,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ
);
CREATE INDEX idx_eval_runs_user ON eval_runs (user_id, created_at DESC);
CREATE INDEX idx_eval_runs_dataset ON eval_runs (dataset_id);

CREATE TABLE eval_attempts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id UUID NOT NULL REFERENCES eval_runs(id) ON DELETE CASCADE,
    task_id UUID NOT NULL,
    -- The LLM config tested (an `eval_runs.models` id).
    model_id UUID NOT NULL,
    attempt_no INT NOT NULL,
    -- The router's attempt tag, `<run>:<attempt id>` (`trace_usage.eval_attempt`).
    tag TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'done', 'failed', 'cancelled')),
    -- The task as it was when the run started: a past run never changes.
    instance JSONB NOT NULL,
    -- The graded record (`AttemptRecord`), with the router's cost once it is attached.
    record JSONB,
    -- The model's patch as graded (test files and inline test edits removed).
    patch TEXT,
    -- Object-store keys of the agent's and the grader's logs and the agent's session export.
    artifacts JSONB NOT NULL DEFAULT '{}',
    error TEXT,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    UNIQUE (run_id, task_id, model_id, attempt_no)
);
CREATE INDEX idx_eval_attempts_run ON eval_attempts (run_id);

CREATE TABLE eval_jobs (
    id BIGSERIAL PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN (
        'harvest', 'task_rewrite', 'task_build', 'task_screen',
        'run_preflight', 'run_attempt', 'run_finalize'
    )),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    dataset_id UUID REFERENCES eval_datasets(id) ON DELETE CASCADE,
    task_id UUID REFERENCES eval_tasks(id) ON DELETE CASCADE,
    run_id UUID REFERENCES eval_runs(id) ON DELETE CASCADE,
    attempt_id UUID REFERENCES eval_attempts(id) ON DELETE CASCADE,
    -- For `run_preflight`: the LLM config to check.
    model_id UUID,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'done', 'failed')),
    tries INT NOT NULL DEFAULT 0,
    max_tries INT NOT NULL DEFAULT 2,
    -- Refreshed while a server works on the job; a stale one is taken over.
    heartbeat_at TIMESTAMPTZ,
    run_after TIMESTAMPTZ NOT NULL DEFAULT now(),
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);
CREATE INDEX idx_eval_jobs_pending ON eval_jobs (run_after) WHERE status = 'pending';
CREATE INDEX idx_eval_jobs_running ON eval_jobs (heartbeat_at) WHERE status = 'running';
CREATE INDEX idx_eval_jobs_dataset ON eval_jobs (dataset_id) WHERE dataset_id IS NOT NULL;
CREATE INDEX idx_eval_jobs_run ON eval_jobs (run_id) WHERE run_id IS NOT NULL;
-- A run is finalized once: the last two attempts to end may both try to add the job.
CREATE UNIQUE INDEX idx_eval_jobs_one_finalize ON eval_jobs (run_id) WHERE kind = 'run_finalize';
