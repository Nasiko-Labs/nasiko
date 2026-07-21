-- mcp_agent_connector_access was keyed (user_id, agent_id, connector_id) —
-- per-CALLER, not per-agent. Two different people who can manage the same
-- agent got independent Allow/Block state for its tools: if the owner
-- blocked a tool, a co-manager using the same agent was unaffected (their own
-- row, or a default-allow with no row, still applied). Fix: this is a
-- property of the AGENT, set once, applying to every caller uniformly.
--
-- Before running this in any environment with real data, check for actual
-- conflicting rows:
--   SELECT agent_id, connector_id, COUNT(DISTINCT user_id)
--   FROM mcp_agent_connector_access GROUP BY 1,2 HAVING COUNT(DISTINCT user_id) > 1;
-- Local dev DB had zero rows total when this migration was authored, so the
-- collapse step below is exercised but not proven against real conflicts —
-- re-verify in staging/production before relying on it there.

-- Step 1: collapse duplicate (agent_id, connector_id) rows to one, using
-- most-restrictive-wins: enabled=false beats true, and among tool_rules the
-- most restrictive stance per (pattern, stance) pair is kept — block > ask >
-- allow. This never silently loosens an existing restriction; worst case a
-- manager sees a tool blocked that they can then explicitly re-allow.
WITH ranked AS (
    SELECT
        id, agent_id, connector_id, enabled, tool_rules,
        ROW_NUMBER() OVER (
            PARTITION BY agent_id, connector_id
            ORDER BY enabled ASC, id
        ) AS rn
    FROM mcp_agent_connector_access
),
winners AS (
    SELECT r.id, r.agent_id, r.connector_id
    FROM ranked r
    WHERE r.rn = 1
),
merged_rules AS (
    SELECT
        a.agent_id,
        a.connector_id,
        jsonb_agg(DISTINCT rule) AS tool_rules
    FROM mcp_agent_connector_access a,
         LATERAL jsonb_array_elements(a.tool_rules) AS rule
    GROUP BY a.agent_id, a.connector_id
),
stance_priority AS (
    SELECT
        m.agent_id,
        m.connector_id,
        (
            SELECT jsonb_agg(jsonb_build_object('pattern', pattern, 'stance', best_stance))
            FROM (
                SELECT
                    r ->> 'pattern' AS pattern,
                    (array_agg(r ->> 'stance' ORDER BY
                        CASE r ->> 'stance' WHEN 'block' THEN 0 WHEN 'ask' THEN 1 ELSE 2 END
                    ))[1] AS best_stance
                FROM jsonb_array_elements(m.tool_rules) AS r
                GROUP BY r ->> 'pattern'
            ) collapsed
        ) AS tool_rules
    FROM merged_rules m
)
UPDATE mcp_agent_connector_access a
SET enabled = (
        SELECT bool_and(x.enabled) FROM mcp_agent_connector_access x
        WHERE x.agent_id = a.agent_id AND x.connector_id = a.connector_id
    ),
    tool_rules = COALESCE(
        (SELECT sp.tool_rules FROM stance_priority sp
         WHERE sp.agent_id = a.agent_id AND sp.connector_id = a.connector_id),
        '[]'::jsonb
    )
FROM winners w
WHERE a.id = w.id;

DELETE FROM mcp_agent_connector_access a
WHERE NOT EXISTS (
    SELECT 1 FROM (
        SELECT id, agent_id, connector_id,
               ROW_NUMBER() OVER (PARTITION BY agent_id, connector_id ORDER BY enabled ASC, id) AS rn
        FROM mcp_agent_connector_access
    ) w WHERE w.id = a.id AND w.rn = 1
);

-- Step 2: drop user_id from the table's identity.
ALTER TABLE mcp_agent_connector_access
    DROP CONSTRAINT mcp_agent_connector_access_user_id_agent_id_connector_id_key;
ALTER TABLE mcp_agent_connector_access
    DROP CONSTRAINT mcp_agent_connector_access_user_id_fkey;
DROP INDEX IF EXISTS idx_mcp_agent_connector_access_user_agent;

ALTER TABLE mcp_agent_connector_access DROP COLUMN user_id;
ALTER TABLE mcp_agent_connector_access
    ADD CONSTRAINT uq_mcp_agent_connector_access UNIQUE (agent_id, connector_id);
CREATE INDEX idx_mcp_agent_connector_access_agent ON mcp_agent_connector_access(agent_id);

COMMENT ON TABLE mcp_agent_connector_access IS
    'Per-agent permission override, shared by every caller who manages the agent. '
    'The single most important rule: NO ROW = fully allowed. A row only ever exists '
    'to restrict — disable the whole connector for the agent, or apply per-tool '
    'allow/ask/block rules. tool_rules is a JSONB array of {pattern, stance}; app '
    'code validates/dedupes it on write. This table must never be consulted on its '
    'own: every access check confirms connector reachability (owner/grant) FIRST, '
    'so a stale enabled=true row can never re-admit a revoked grant.';
