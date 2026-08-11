-- Allow sharing an MCP connector directly with a specific AGENT, independent
-- of who owns it — grant_type = 'agent', grantee_id = the agent's UUID as
-- text. Lets whoever manages that agent enable the connector for it (`PUT
-- /api/mcp/agents/{agent_id}/connectors/{connector_id}`) even without their
-- own personal reachability to the connector otherwise.
--
-- grantee_id stays TEXT (an agent UUID as text for this type, the '*' sentinel
-- for public). The public-sentinel CHECK already permits agent rows
-- (grant_type <> 'public' AND grantee_id <> '*'), so only grant_type needs
-- relaxing — mirrors ee/migrations/1021_mcp_connector_grants_org.sql for
-- team/department.
--
-- OSS migrations always run before EE ones, but on a database that has
-- ALREADY run EE's migrations before this one was added (any real upgrade,
-- not just a from-scratch install), 'team'/'department' grant rows may
-- already exist — a naive `CHECK (grant_type IN ('user','public','agent'))`
-- would immediately violate them. OSS can't reference those EE-only type
-- names by string literal either way (see CLAUDE.md's OSS/EE boundary), so
-- instead of hardcoding a replacement list, union 'agent' onto whatever
-- grant_type values are ALREADY present in the table — safe and
-- boundary-respecting regardless of migration history or edition.
DO $$
DECLARE
    allowed text[];
    check_sql text;
BEGIN
    SELECT array_agg(DISTINCT grant_type) INTO allowed FROM mcp_connector_grants;
    allowed := ARRAY(SELECT DISTINCT unnest(coalesce(allowed, '{}') || ARRAY['user', 'public', 'agent']));

    ALTER TABLE mcp_connector_grants DROP CONSTRAINT IF EXISTS chk_mcp_grants_grant_type;

    check_sql := 'ALTER TABLE mcp_connector_grants ADD CONSTRAINT chk_mcp_grants_grant_type CHECK (grant_type IN ('
        || (SELECT string_agg(quote_literal(v), ', ') FROM unnest(allowed) v)
        || '))';
    EXECUTE check_sql;
END $$;

-- Expression index for the UUID-valued grantee_id lookups
-- (`agent_has_connector_grant`/`list_agent_granted_connectors` filter
-- `grantee_id = $agent_id` directly as text, so this is a plain btree, not the
-- `::uuid` cast index team/department need).
CREATE INDEX IF NOT EXISTS idx_mcp_grants_grantee_agent
    ON mcp_connector_grants (grantee_id)
    WHERE grant_type = 'agent';
