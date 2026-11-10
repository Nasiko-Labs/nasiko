-- Harness sessions and agent parking policy.
--
-- A "harness" is a platform-built coding-agent image (OpenCode, Claude Code,
-- Codex, ...) that speaks ACP through a shim and runs in a per-session sandbox.
-- This migration adds the schema for it: a harness catalog, the agent columns
-- that point at it, the parking policy that decides whether an agent may be
-- scaled to zero, and the session columns a suspended sandbox needs in order to
-- be resumed. Everything here is additive and defaults existing rows to
-- today's behavior.

-- 1. Harness catalog.
--
-- The platform used to guess which harness an agent was from its image name,
-- which breaks as soon as an image is mirrored, retagged or rebuilt under
-- another name, and leaves nowhere to record how to launch it. The catalog
-- makes the harness kind explicit data: which image to run, which argv the ACP
-- shim executes inside it, which ACP major version it speaks, and the minimum
-- resources it needs to start. `enabled` lets an operator withdraw a harness
-- without deleting rows that agents reference.
CREATE TABLE IF NOT EXISTS harness_catalog (
    -- Slug, e.g. 'opencode', 'claude-code', 'codex'. Deliberately distinct from
    -- the agents.coding_agent_integration_id slugs ('claude' vs 'claude-code');
    -- the server owns the mapping between the two.
    id               TEXT PRIMARY KEY,
    -- Display name shown in the UI.
    name             TEXT NOT NULL,
    -- Fully-qualified OCI image of the platform-built harness image.
    image            TEXT NOT NULL,
    -- argv the shim runs to start the harness's ACP server.
    acp_command      TEXT[] NOT NULL CHECK (cardinality(acp_command) > 0),
    -- ACP major version the harness speaks.
    protocol_version INTEGER NOT NULL DEFAULT 1,
    min_cpu_milli    INTEGER NOT NULL DEFAULT 500,
    min_memory       TEXT NOT NULL DEFAULT '1Gi',
    enabled          BOOLEAN NOT NULL DEFAULT true,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

DROP TRIGGER IF EXISTS trg_harness_catalog_updated_at ON harness_catalog;
CREATE TRIGGER trg_harness_catalog_updated_at BEFORE UPDATE ON harness_catalog
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- 2. `agents` changes.
--
-- 2a. Widen the workload_kind CHECK to include 'harness'.
--
-- 0063 declared the CHECK inline, so Postgres named it <table>_<column>_check.
-- Naming it explicitly here makes the next widening a one-line change.
-- ADD CONSTRAINT validates the table under ACCESS EXCLUSIVE, which is fine for
-- `agents` (small).
ALTER TABLE agents DROP CONSTRAINT IF EXISTS agents_workload_kind_check;
ALTER TABLE agents ADD CONSTRAINT agents_workload_kind_check
    CHECK (workload_kind IN ('agent', 'mcp_connector', 'acp_coding_agent', 'harness'));

-- 2b. Which harness a 'harness' agent runs. NULL for every other kind.
ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS harness_id TEXT REFERENCES harness_catalog(id);
ALTER TABLE agents DROP CONSTRAINT IF EXISTS agents_harness_id_only_for_harness;
ALTER TABLE agents ADD CONSTRAINT agents_harness_id_only_for_harness
    CHECK (harness_id IS NULL OR workload_kind = 'harness');

-- 2c. Parking policy: may the platform park (scale to zero) this agent when idle?
-- 'auto' lets the idle reaper park it; 'resident' keeps it always on.
-- Existing fleets stay always-on (parking is opt-in); only agents created from
-- here on default to 'auto'. Adding the column with a 'resident' default is
-- metadata-only and backfills every existing row, then the default is swapped.
ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS parking_policy TEXT NOT NULL DEFAULT 'resident'
        CHECK (parking_policy IN ('auto', 'resident'));
ALTER TABLE agents ALTER COLUMN parking_policy SET DEFAULT 'auto';

-- 3. `chat_sessions` changes.
--
-- The runtime_state / runtime_handle / last_turn_at columns already exist
-- (0063); these add what a provider-backed, suspendable sandbox needs.
ALTER TABLE chat_sessions
    -- "<provider>:<backend_ref>" as returned by the SandboxProvider. Replaces
    -- the free-form runtime_handle for provider-backed sessions; runtime_handle
    -- stays in place for the rows that still use it.
    ADD COLUMN IF NOT EXISTS sandbox_ref TEXT,
    -- When the sandbox was suspended; NULL while it is live or never suspended.
    ADD COLUMN IF NOT EXISTS suspended_at TIMESTAMPTZ,
    -- Git URL the session's workspace was cloned from, if any.
    ADD COLUMN IF NOT EXISTS workspace_repo TEXT,
    -- The approval a turn is blocked on ({hitl_id, tool_call_id}), set when the wait begins and
    -- cleared when it ends. It is recorded for a future resume; nothing re-delivers it yet.
    ADD COLUMN IF NOT EXISTS pending_permission JSONB;

-- (The partial index the idle reaper scans, `idx_chat_sessions_reapable`, is built concurrently
-- in 0085: it indexes an existing, large table and must not hold a write lock on it.)

-- 4. Launch harnesses.
--
-- The image tags are placeholders; the release process pins the real ones. The
-- per-harness `agents` rows are not created here -- the server seeds them at
-- boot.
INSERT INTO harness_catalog (id, name, image, acp_command) VALUES
    ('opencode',    'OpenCode',    'nasiko/harness-opencode:0.1.0',     '{opencode,acp}'),
    ('claude-code', 'Claude Code', 'nasiko/harness-claude-code:0.1.0',  '{claude-code-acp}'),
    ('codex',       'Codex',       'nasiko/harness-codex:0.1.0',        '{codex-acp}')
ON CONFLICT (id) DO NOTHING;
