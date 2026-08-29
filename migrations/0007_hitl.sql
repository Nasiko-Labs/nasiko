-- Human-in-the-Loop (HITL) persistence core.

CREATE TABLE hitl_requests (
  id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),

  kind                     TEXT NOT NULL CHECK (kind IN
                              ('input_required','auth_required','tool_approval')),
  origin                   TEXT NOT NULL CHECK (origin IN
                              ('direct_chat','agent_proxy','orchestrator','maf','mcp_tool')),
  status                   TEXT NOT NULL DEFAULT 'pending' CHECK (status IN
                              ('pending','resolved','rejected','expired','canceled')),
  -- Delivery state of the human's decision to the paused execution. An attempt is in flight
  -- when resume_claimed_at IS NOT NULL AND resume_status = 'not_started' (the lease lives
  -- there, not here). completed = peer confirmed receipt; failed = attempts exhausted or
  -- non-retryable; delivery_outcome_unknown = lease expired mid-attempt (set by the recovery
  -- sweep, never auto-retried).
  resume_status            TEXT NOT NULL DEFAULT 'not_started' CHECK (resume_status IN
                              ('not_started','completed','failed','delivery_outcome_unknown')),

  -- ON DELETE CASCADE is deliberate; agent foreign keys use mixed deletion policies
  -- elsewhere (agent_builds cascades; chat_sessions uses SET NULL; session_traces uses
  -- SET NULL plus a denormalised agent_name so listings survive agent deletion).
  -- Agents are normally soft-deleted, so hard deletion and this cascade are rare;
  -- HITL history is intentionally scoped to its agent.
  agent_id                 UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,

  -- The user this execution/action is attributed to, and the authorization decision for every
  -- kind (including tool_approval, whose approver is the delegating user, not whoever manages
  -- the agent — connector credentials are always the calling user's own).
  owner_user_id            UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  resolved_by              UUID REFERENCES users(id),

  task_id                  TEXT,                        -- A2A taskId of the paused agent's task
  context_id               TEXT,                        -- A2A contextId; required for kind=tool_approval too
                                                          -- (approval matching is scoped to one conversation)
  chat_session_id          TEXT REFERENCES chat_sessions(session_id),  -- origin=orchestrator only
  maf_execution_id         UUID REFERENCES maf_executions(id) ON DELETE SET NULL, -- origin=maf only
  maf_step_index           INT,                                                   -- origin=maf only

  -- kind=tool_approval only. connector_id/tool_name are part of the approval's matching identity
  -- (agent_id, connector_id, tool_name, context_id) — see uq_hitl_pending_per_tool_call below.
  connector_id             UUID,
  tool_name                TEXT,
  -- Audit-only: recorded for visibility at approval time, but not part of the matching key —
  -- a retried tool call is matched by (agent_id, connector_id, tool_name, context_id), not by
  -- hashing arguments, since an agent's retry may regenerate slightly different arguments.
  arguments_hash           TEXT,
  consumed_at              TIMESTAMPTZ,                 -- kind=tool_approval only, one-time-use claim

  -- Postgres treats NULLs as distinct in unique indexes, so uq_hitl_pending_per_tool_call below
  -- enforces nothing if any of these three is NULL on a tool_approval row. This CHECK is the
  -- backstop for that, independent of whatever the (not-yet-written) per-origin constructor does.
  CONSTRAINT chk_hitl_tool_approval_identity CHECK (
    kind <> 'tool_approval'
    OR (connector_id IS NOT NULL AND tool_name IS NOT NULL AND context_id IS NOT NULL)
  ),

  -- Split deliberately: question (write-once, at creation), human_response (written only
  -- by resolve()), resume_state (written only by the resume dispatcher, never API-exposed).
  question                 JSONB NOT NULL,
  human_response            JSONB,
  resume_state              JSONB NOT NULL DEFAULT '{}',

  -- Delivery lease for the resume dispatcher (mirrors build_jobs.picked_at). NULL = unclaimed.
  -- Claim (atomic; the only mutual-exclusion mechanism):
  --   UPDATE hitl_requests
  --      SET resume_claimed_at = now(), resume_dispatch_attempts = resume_dispatch_attempts + 1
  --    WHERE id = $1 AND status = 'resolved' AND resume_status = 'not_started'
  --      AND (resume_claimed_at IS NULL OR resume_claimed_at < now() - interval '2 minutes')
  --   RETURNING *;
  resume_claimed_at         TIMESTAMPTZ,
  resume_dispatch_attempts  INT NOT NULL DEFAULT 0,
  resume_last_error         TEXT,

  created_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at                TIMESTAMPTZ,
  resolved_at                TIMESTAMPTZ
);

CREATE TRIGGER trg_hitl_requests_updated_at
  BEFORE UPDATE ON hitl_requests FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- At most one OPEN pause per A2A task (many historical resolved/rejected/expired/canceled
-- rows may legitimately share a task_id).
CREATE UNIQUE INDEX uq_hitl_pending_per_task
  ON hitl_requests (task_id) WHERE status = 'pending' AND task_id IS NOT NULL;

-- At most one open approval per (agent, connector, tool, conversation) — MCP idempotent-creation
-- guard. Keyed on tool identity, not on argument content: an agent's retry of the same call may
-- carry regenerated (non-identical) arguments, and two different tools with coincidentally
-- identical arguments must never collide on this index.
CREATE UNIQUE INDEX uq_hitl_pending_per_tool_call
  ON hitl_requests (agent_id, connector_id, tool_name, context_id)
  WHERE status = 'pending' AND kind = 'tool_approval';

CREATE INDEX idx_hitl_pending_owner  ON hitl_requests (owner_user_id) WHERE status = 'pending';
CREATE INDEX idx_hitl_maf_execution  ON hitl_requests (maf_execution_id) WHERE maf_execution_id IS NOT NULL;
CREATE INDEX idx_hitl_resume_pending ON hitl_requests (resume_status) WHERE status = 'resolved';
