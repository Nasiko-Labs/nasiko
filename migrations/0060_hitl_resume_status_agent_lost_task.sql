-- Add 'agent_lost_task' to `hitl_requests.resume_status`'s allowed values.
--
-- A paused A2A task lives in the agent's own memory. When the agent restarts, or the human's
-- answer reaches a replica that never saw the task, the agent replies that the task is unknown
-- (A2A `TaskNotFoundError`, JSON-RPC code -32001; a 404 counts only when its body carries that
-- code). The resume dispatcher (`oss/server/src/hitl/mod.rs`) re-delivers the answer a bounded
-- number of times (`HITL_LOST_TASK_RETRIES`), so a load balancer can still reach the replica that
-- holds the task; only when every attempt answers -32001 does the row become 'agent_lost_task'.
-- It is terminal: the answer reached no task, and the requester is told to ask again.
--
-- Same forward-only shape as 0027_hitl_resume_status_skipped.sql: drop the constraint and re-add
-- it with the full list (sqlx hashes applied migration files, so 0007/0027 cannot be edited).
-- Rows already in any other state are untouched; this only widens what is allowed going forward.

ALTER TABLE hitl_requests DROP CONSTRAINT IF EXISTS hitl_requests_resume_status_check;
ALTER TABLE hitl_requests
  ADD CONSTRAINT hitl_requests_resume_status_check CHECK (resume_status IN
    ('not_started', 'completed', 'failed', 'delivery_outcome_unknown', 'skipped', 'agent_lost_task'));
