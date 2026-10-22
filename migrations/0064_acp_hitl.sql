-- =============================================================================
-- ACP coding-agent HITL (NAS-842)
--
-- Two additive changes so a coding agent's ACP `session/request_permission` can
-- be gated by a human:
--
--   1. A new `coding_agent` value for the `hitl_requests.origin` CHECK. A coding
--      agent's permission request is a blocked, live ACP request on the open
--      connection (no A2A task, no resume dispatcher) — the turn's permission
--      handler awaits the `hitl_requests` row and answers the agent when a human
--      resolves it. Mirrors `oss/hitl/src/types.rs` (`HitlOrigin::CodingAgent`).
--
--   2. A per-agent `hitl_enabled` flag. Off by default, so every coding agent
--      keeps running in auto-approve mode until opted in — set at deploy
--      (`--hitl`) and read back like `minimal_code_enabled`. When on, the turn
--      drives the agent with a delegate that routes permission requests to HITL
--      and injects `permission:"ask"` into the agent's OpenCode config.
-- =============================================================================

-- 1. Extend the origin CHECK. The inline constraint from 0007_hitl.sql is
--    auto-named `hitl_requests_origin_check`; drop and re-add it with the new value.
ALTER TABLE hitl_requests DROP CONSTRAINT hitl_requests_origin_check;
ALTER TABLE hitl_requests ADD CONSTRAINT hitl_requests_origin_check
    CHECK (origin IN
        ('direct_chat', 'agent_proxy', 'orchestrator', 'maf', 'mcp_tool', 'coding_agent'));

-- 2. Exempt `coding_agent` from the tool-approval identity CHECK. That CHECK
--    (0007_hitl.sql) requires every `tool_approval` row to carry a
--    `connector_id` + `tool_name` + `context_id` — the identity of an *MCP* tool
--    call, and the backstop for `uq_hitl_pending_per_tool_call`'s
--    connector-keyed uniqueness. A coding-agent ACP permission has no MCP
--    connector; its identity is the agent + session (`context_id`), and each
--    request is distinct (no pending-dedup), so it only needs `context_id`.
ALTER TABLE hitl_requests DROP CONSTRAINT chk_hitl_tool_approval_identity;
ALTER TABLE hitl_requests ADD CONSTRAINT chk_hitl_tool_approval_identity CHECK (
    kind <> 'tool_approval'
    OR (origin = 'coding_agent' AND context_id IS NOT NULL)
    OR (connector_id IS NOT NULL AND tool_name IS NOT NULL AND context_id IS NOT NULL)
);

-- 3. Per-agent HITL opt-in (mirrors 0042_agent_minimal_code_flag.sql).
ALTER TABLE agents ADD COLUMN hitl_enabled BOOLEAN NOT NULL DEFAULT false;
