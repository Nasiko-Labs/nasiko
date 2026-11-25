-- Replay evals: a dedicated `eval` coding-agent identity, and the attempt tag its calls are
-- filed under.
--
-- An eval run replays coding tasks through the LLM router with an agent of its own, once per
-- attempt, and then reads back what each attempt cost (`GET /api/evals/usage`). Its calls carry no
-- flow, so the router writes each one as its own `direct:<uuid>` row; the attempt the runner named
-- in the `x-nasiko-eval-attempt` header is what groups them. The router keeps the header only for
-- the `eval` integration, so no other agent can file spend under someone's run.
--
-- `eval` joins the integrations an agent row may be. The check was declared inline in 0019, so
-- Postgres named it `agents_coding_agent_integration_id_check`; it is replaced, not altered (a
-- check cannot be). The unique index from 0019 is unchanged: one live `eval` identity per owner.
--
-- cold: compatible (`eval_attempt` is a hot-only column, like the router_* share in 0079 and the
-- failure counters in 0095: tiering copies named columns, and the cold copy and the rollups keep
-- the spend it tags; an attempt is read back within minutes of its run, never after tiering)

ALTER TABLE agents
    DROP CONSTRAINT agents_coding_agent_integration_id_check,
    ADD CONSTRAINT agents_coding_agent_integration_id_check
        CHECK (coding_agent_integration_id IN ('claude', 'opencode', 'codex', 'cursor', 'eval'));

ALTER TABLE trace_usage
    -- `<run>:<task>:<attempt>` as the eval runner named it, 1-128 characters of
    -- `A-Z a-z 0-9 : . _ / -` (checked by the router). NULL on every other row.
    ADD COLUMN eval_attempt TEXT;
