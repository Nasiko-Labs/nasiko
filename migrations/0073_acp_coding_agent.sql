-- ACP coding-agent support (squashed): catalog workload kind, session
-- lifecycle bookkeeping, and the session-token revocation list.
--
-- These three concerns were developed as separate feature-branch migrations
-- (agents.workload_kind, chat_sessions lifecycle, revoked_session_tokens) and
-- are squashed into one before merge so the branch adds a single, coherent
-- migration on top of development's schema rather than three that collided on
-- version numbers. Everything here is additive and defaults existing rows to
-- today's behavior.

-- 1. Catalog workload kind (NAS-838/NAS-840).
--
-- The agent proxy needs to know, at request time, whether an agent is an
-- ordinary HTTP/A2A agent or an ACP coding agent — the two are driven
-- completely differently (reverse-proxy an HTTP endpoint vs. open a stdio turn).
-- `WorkloadKind` existed only at the runtime layer and was never stored, so the
-- proxy had no way to tell them apart. This adds the column it branches on.
-- Defaults to 'agent' so every existing row keeps today's behavior; the values
-- mirror the runtime `WorkloadKind` variants.
ALTER TABLE agents
    ADD COLUMN workload_kind TEXT NOT NULL DEFAULT 'agent'
        CHECK (workload_kind IN ('agent', 'mcp_connector', 'acp_coding_agent'));

-- 2. Coding-agent session lifecycle (NAS-838).
--
-- An ACP coding-agent session needs runtime bookkeeping the ordinary chat
-- session never did: which container currently serves it, what state that
-- container is in, and a single-turn-at-a-time lock so two control-plane
-- replicas (or two sharers of one session) can't drive the same agent process
-- at once. This is tracked on the existing `chat_sessions` row rather than a
-- separate registry service, so these columns are additive and default to the
-- "nothing deployed yet" state for every existing row.
ALTER TABLE chat_sessions
    -- The agent's own session id from `session/new`. NULL until the first turn
    -- opens a session; required later to `session/load` on the cold path.
    ADD COLUMN acp_session_id TEXT,
    -- Backend handle for the session's container: Docker container id, or K8s
    -- object name. The runtime itself is a single deployment-wide choice
    -- (`AGENT_RUNTIME`), so it is not recorded per session — the turn path drives
    -- every runtime the same way through the `ContainerRuntime` trait.
    ADD COLUMN runtime_handle TEXT,
    -- Where the container is in its lifecycle. 'parked' = no container running
    -- (the default and the cold-start state); 'starting' = being brought up;
    -- 'running' = live and serving; 'stopping' = being torn down.
    ADD COLUMN runtime_state TEXT NOT NULL DEFAULT 'parked'
        CHECK (runtime_state IN ('parked', 'starting', 'running', 'stopping')),
    -- PVC name / Docker volume holding this session's workspace + agent state.
    ADD COLUMN volume_ref TEXT,
    -- The one-active-turn lock. `turn_owner` names the control-plane replica
    -- holding the current turn; `lease_expires_at` is when that lock lapses so a
    -- dead replica can't hold it forever; `turn_fence` is a monotonically
    -- increasing token bumped on every acquire, so a resumed-from-the-dead
    -- holder can be fenced out (its stale fence no longer matches the row).
    ADD COLUMN turn_owner TEXT,
    ADD COLUMN lease_expires_at TIMESTAMPTZ,
    ADD COLUMN turn_fence BIGINT NOT NULL DEFAULT 0,
    -- Last turn end, in UTC — drives the idle shutdown decision.
    ADD COLUMN last_turn_at TIMESTAMPTZ,
    -- The turn currently running, so a crash can mark exactly that turn failed.
    ADD COLUMN active_turn_id TEXT;

-- Startup reconciliation scans for sessions whose row says a container is live
-- ('starting'/'running'/'stopping') to check them against what's actually
-- running and reset the stragglers. A partial index keeps that scan cheap even
-- when the vast majority of chat sessions are plain (non-coding) and 'parked'.
CREATE INDEX idx_chat_sessions_active_runtime
    ON chat_sessions (runtime_state)
    WHERE runtime_state <> 'parked';

-- 3. Session-token revocation list (NAS-839).
--
-- A session token is a short-lived, signed credential carrying
-- {agent_id, user_id, session_id, flow_id, jti}. Verification is offline (the
-- signature is the proof), so to revoke a token before its `exp` the control
-- plane records its `jti` here; the MCP gateway and LLM router reject any token
-- whose `jti` is present. Rows are safe to purge once `expires_at` has passed
-- (the token can no longer verify regardless), so the table stays small.
CREATE TABLE IF NOT EXISTS revoked_session_tokens (
    jti        TEXT PRIMARY KEY,
    revoked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- The token's own `exp`, so a sweeper can drop entries that can no longer verify.
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_revoked_session_tokens_expires_at
    ON revoked_session_tokens (expires_at);
