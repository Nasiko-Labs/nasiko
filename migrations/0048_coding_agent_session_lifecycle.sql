-- Coding-agent session lifecycle.
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
    -- Which runtime backs this session's container: 'docker' or 'k8s'.
    ADD COLUMN runtime_kind TEXT,
    -- Backend handle for that container: Docker container id, or K8s object name.
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
