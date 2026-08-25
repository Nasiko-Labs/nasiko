-- Human-in-the-Loop (HITL) persistence core.
-- See docs/HITL_IMPLEMENTATION_PLAN.md §4 for the full design rationale.

CREATE TABLE hitl_requests (
  id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),

  kind                     TEXT NOT NULL CHECK (kind IN
                              ('input_required','auth_required','tool_approval')),
  origin                   TEXT NOT NULL CHECK (origin IN
                              ('direct_chat','agent_proxy','orchestrator','maf','mcp_tool')),
  status                   TEXT NOT NULL DEFAULT 'pending' CHECK (status IN
                              ('pending','resolved','rejected','expired','canceled')),
  resume_status            TEXT NOT NULL DEFAULT 'not_started' CHECK (resume_status IN
                              ('not_started','dispatching','dispatched','failed',
                               'delivery_outcome_unknown')),

  agent_id                 UUID NOT NULL,               -- the paused/gated agent

  -- The user this execution/action is attributed to. NOT, by itself, the
  -- authorization decision for every kind — see docs/HITL_IMPLEMENTATION_PLAN.md §10.
  owner_user_id            UUID NOT NULL,
  resolved_by              UUID REFERENCES users(id),

  task_id                  TEXT,                        -- A2A taskId of the paused agent's task
  context_id               TEXT,                        -- A2A contextId of that task
  chat_session_id          TEXT REFERENCES chat_sessions(session_id),  -- origin=orchestrator only
  maf_execution_id         UUID REFERENCES maf_executions(id) ON DELETE SET NULL, -- origin=maf only
  maf_step_index           INT,                                                   -- origin=maf only
  arguments_hash           TEXT,                        -- kind=tool_approval only
  consumed_at              TIMESTAMPTZ,                 -- kind=tool_approval only

  -- Split deliberately: question (write-once, at creation), human_response (written only
  -- by resolve()), resume_state (written only by the resume dispatcher, never API-exposed).
  question                 JSONB NOT NULL,
  human_response           JSONB,
  resume_state              JSONB NOT NULL DEFAULT '{}',

  resume_dispatch_attempts  INT NOT NULL DEFAULT 0,
  resume_last_error         TEXT,

  created_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at                TIMESTAMPTZ,
  resolved_at                TIMESTAMPTZ
);

-- At most one OPEN pause per A2A task (many historical resolved/rejected/expired/canceled
-- rows may legitimately share a task_id — see docs/HITL_IMPLEMENTATION_PLAN.md §3.5).
CREATE UNIQUE INDEX uq_hitl_pending_per_task
  ON hitl_requests (task_id) WHERE status = 'pending' AND task_id IS NOT NULL;

-- At most one open pause per (agent, tool-call shape, context) — MCP idempotent-creation guard.
CREATE UNIQUE INDEX uq_hitl_pending_per_tool_call
  ON hitl_requests (agent_id, arguments_hash, context_id)
  WHERE status = 'pending' AND kind = 'tool_approval';

CREATE INDEX idx_hitl_pending_owner  ON hitl_requests (owner_user_id) WHERE status = 'pending';
CREATE INDEX idx_hitl_maf_execution  ON hitl_requests (maf_execution_id) WHERE maf_execution_id IS NOT NULL;
CREATE INDEX idx_hitl_resume_pending ON hitl_requests (resume_status) WHERE status = 'resolved';
