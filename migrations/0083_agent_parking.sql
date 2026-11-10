-- Agent parking: schema for parking an agent (no running sandbox while idle).
--
-- A parked agent keeps its image and its `agents` row at rest; a sandbox is
-- created only while it serves a request. This migration adds the status the
-- row rests in and the two columns the parking decision and the idle reaper
-- read. Everything here is additive; nothing changes for rows that never park.

-- 1. A new resting status.
--
-- 0011 declared agents_status_check with an explicit name, so widening it is a
-- drop and re-add under that name (the same pattern 0076 2a uses for
-- workload_kind). ADD CONSTRAINT validates the table under ACCESS EXCLUSIVE,
-- which is fine for `agents` (small).
ALTER TABLE agents DROP CONSTRAINT IF EXISTS agents_status_check;
ALTER TABLE agents ADD CONSTRAINT agents_status_check
    CHECK (status IN ('registered', 'deploying', 'running', 'stopped', 'failed', 'crashed', 'parked'));

-- 2. Whether the agent's image can run as a sandbox.
--
-- NULL means it has not been probed yet; true/false is the probe's verdict.
-- Kept separate from parking_policy (0076): the policy is the owner's choice,
-- this is a fact about the image.
ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS sandbox_compatible BOOLEAN;

-- 3. When the agent last served a request.
--
-- The idle signal for a parked-agent reaper: a woken agent whose last request
-- is older than the idle window is parked again. Harness sessions use
-- chat_sessions.last_turn_at instead; this column is for plain agents.
ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS last_request_at TIMESTAMPTZ;
