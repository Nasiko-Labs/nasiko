-- agent_deployments: RESTRICT → CASCADE
ALTER TABLE agent_deployments
    DROP CONSTRAINT agent_deployments_agent_id_fkey,
    ADD CONSTRAINT agent_deployments_agent_id_fkey
        FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE;

-- proxy_logs: implicit RESTRICT → CASCADE
ALTER TABLE proxy_logs
    DROP CONSTRAINT proxy_logs_target_agent_id_fkey,
    ADD CONSTRAINT proxy_logs_target_agent_id_fkey
        FOREIGN KEY (target_agent_id) REFERENCES agents(id) ON DELETE CASCADE;
