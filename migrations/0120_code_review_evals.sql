-- Code review evals (`/api/evals/code-review/*`).
--
-- A task is one GitHub pull request read at a fixed base/head commit, with its diff and the
-- touched files' text at head kept so a past task reads the same forever. `reference_findings`
-- is empty until a person adds validated ground truth (`graded` flips to true); a task with no
-- reference findings may still be reviewed, but its attempts are an ungraded preview, never a
-- scored benchmark result (the brief: "a single PR without reference findings ... must not be
-- treated as a trustworthy benchmark result").
--
-- A run reviews one task on chosen models (each one of the caller's own LLM configs); each
-- attempt is a single router-attributed chat completion (`nasiko eval` style tag,
-- `cr-<run>:<model>`, read back from `trace_usage` the same way 0116 reads eval attempts), graded
-- by deterministic reference-finding matching when the task is graded.
--
-- cold: compatible (none of these tables is tiered)

CREATE TABLE review_tasks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    owner TEXT NOT NULL,
    repo TEXT NOT NULL,
    pr_number BIGINT NOT NULL,
    pr_title TEXT NOT NULL,
    base_sha TEXT NOT NULL,
    head_sha TEXT NOT NULL,
    diff TEXT NOT NULL,
    -- `{"<path>": "<text at head_sha>"}` for the touched files, the reviewer prompt's context.
    files JSONB NOT NULL DEFAULT '{}',
    -- Human-validated ground truth: `[{file, line, severity, category, description}]`.
    reference_findings JSONB NOT NULL DEFAULT '[]',
    -- True once `reference_findings` is non-empty; an attempt on an ungraded task is a preview.
    graded BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, owner, repo, pr_number)
);
CREATE INDEX idx_review_tasks_user ON review_tasks (user_id, created_at DESC);

CREATE TABLE review_runs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    task_id UUID NOT NULL REFERENCES review_tasks(id) ON DELETE CASCADE,
    -- `[{id, name, provider, model}]`: the LLM configs tested, as they were at start.
    models JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'running'
        CHECK (status IN ('running', 'done', 'failed')),
    -- The report (per-model metrics once every attempt ended), `null` while running.
    report JSONB,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);
CREATE INDEX idx_review_runs_user ON review_runs (user_id, created_at DESC);
CREATE INDEX idx_review_runs_task ON review_runs (task_id);

CREATE TABLE review_attempts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id UUID NOT NULL REFERENCES review_runs(id) ON DELETE CASCADE,
    task_id UUID NOT NULL REFERENCES review_tasks(id) ON DELETE CASCADE,
    -- The LLM config tested (a `review_runs.models` id).
    model_id UUID NOT NULL,
    -- The router's attempt tag, `cr-<run>:<model>` (`trace_usage.eval_attempt`, reusing the same
    -- column and index as replay evals, 0116 — a review attempt is still one tagged router call).
    tag TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'done', 'failed')),
    -- Wall-clock time of the router call that produced `raw_output`.
    latency_ms BIGINT,
    -- The router's own figures for this attempt's one call, read back from `trace_usage` by
    -- `tag`: `{calls, cost_usd, input_tokens, output_tokens, worst_price_source}`. `null` until
    -- the router has billed the call (never estimated client-side).
    usage JSONB,
    -- The model's raw reply, kept for audit even when it failed to parse.
    raw_output TEXT,
    -- Parsed findings: `[{file, line, severity, category, description, confidence}]`.
    findings JSONB,
    -- Per-reference match result, present only when the task is graded.
    matched JSONB,
    -- `{true_positives, false_positives, false_negatives, precision, recall, severity_accuracy}`,
    -- present only when the task is graded.
    metrics JSONB,
    error TEXT,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    UNIQUE (run_id, model_id)
);
CREATE INDEX idx_review_attempts_run ON review_attempts (run_id);
