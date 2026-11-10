-- Non-terminal A2A tasks an agent returned through the proxy. A parkable agent with a row here is
-- busy even when no request is in flight. Rows age out at max(AGENT_OPEN_TASK_MAX_AGE_SECS, effective park TTL + reaper interval).
CREATE TABLE IF NOT EXISTS agent_open_tasks (
    agent_id   UUID        NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    task_id    TEXT        NOT NULL,
    state      TEXT        NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (agent_id, task_id)
);
CREATE INDEX IF NOT EXISTS idx_agent_open_tasks_agent ON agent_open_tasks (agent_id, updated_at);
