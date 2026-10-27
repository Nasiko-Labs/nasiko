-- Per-agent durable state: A2A task snapshots and a small key-value store.
--
-- Agents keep task state in memory by default, so a restart (or a resume on
-- another replica) loses paused work. This gives each agent a platform-held
-- store behind its existing credential (see oss/server/src/agent_state). Rows
-- are scoped by agent, expire, and are encrypted at rest.

-- Whether the agent has registered a durable task store. Written by
-- `POST /api/agent-state/register` when the agent declares the `tasks`
-- capability; a KV-only registration never changes it.
ALTER TABLE agents
    ADD COLUMN state_mode TEXT NOT NULL DEFAULT 'memory'
        CHECK (state_mode IN ('memory', 'durable'));

-- One row per (agent, task). `body` is the agent SDK's serialized task, opaque
-- to the server; it holds the AES-256-GCM ciphertext of its JSON text, not the
-- JSON itself. `version` starts at 1 and increments on every write.
--
-- Versions never restart, so a deleted or expired row is not removed at once: it
-- is emptied into a tombstone (`deleted_at` set, `body` and `context_id` NULL)
-- that keeps its version for one more TTL, and the next write of that task
-- continues from it. The sweeper forgets a tombstone once its own expiry passes.
--
-- ON DELETE CASCADE covers a hard delete of the agent row only. The catalog's
-- delete route is a soft delete (`agents.deleted_at`), which never fires it: the
-- agent's credential stops authenticating the moment it is soft-deleted, and the
-- sweeper (oss/server/src/agent_state/sweeper.rs) removes the rows of deleted
-- agents on its next pass.
CREATE TABLE agent_tasks (
    agent_id    UUID        NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    task_id     TEXT        COLLATE "C" NOT NULL,
    context_id  TEXT,
    state       TEXT        NOT NULL,
    body        TEXT,
    version     BIGINT      NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ NOT NULL,
    deleted_at  TIMESTAMPTZ,
    PRIMARY KEY (agent_id, task_id),
    CHECK ((deleted_at IS NULL) = (body IS NOT NULL))
);
-- Names are compared as bytes ("C"), so listings order the same under any locale
-- and the primary key, like the index below, serves that order and its ranges.
-- Serves `GET /tasks`: newest first, then by id, from a cursor.
CREATE INDEX idx_agent_tasks_listing ON agent_tasks (agent_id, updated_at DESC, task_id);
CREATE INDEX idx_agent_tasks_context ON agent_tasks (agent_id, context_id);
-- Serves the sweeper's `WHERE expires_at <= now() ... LIMIT` scans.
CREATE INDEX idx_agent_tasks_expires ON agent_tasks (expires_at);

-- One row per (agent, key). `value` is ciphertext of the entry's JSON text, as
-- `agent_tasks.body` is, and a deleted or expired entry is a tombstone the same way.
CREATE TABLE agent_kv (
    agent_id    UUID        NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    key         TEXT        COLLATE "C" NOT NULL,
    value       TEXT,
    version     BIGINT      NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ NOT NULL,
    deleted_at  TIMESTAMPTZ,
    PRIMARY KEY (agent_id, key),
    CHECK ((deleted_at IS NULL) = (value IS NOT NULL))
);
-- The primary key serves `GET /kv`: key order, cursor range and prefix, because
-- `key` is a "C" column.
CREATE INDEX idx_agent_kv_expires ON agent_kv (expires_at);
