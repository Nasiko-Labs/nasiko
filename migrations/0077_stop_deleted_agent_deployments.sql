-- Deleting an agent used to tear down its workload but leave its
-- agent_deployments rows at 'running'. Those rows listed as live deployments
-- and kept EE's crash guardian probing workloads that no longer exist. The
-- delete handler now stops them; this converges the rows it left behind.
UPDATE agent_deployments d
SET status = 'stopped', updated_at = now()
FROM agents a
WHERE a.id = d.agent_id
  AND a.deleted_at IS NOT NULL
  AND d.status != 'stopped';
