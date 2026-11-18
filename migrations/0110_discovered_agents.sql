-- A2A agents the CLI found running on a user's machine (`nasiko status`).
--
-- Nasiko never deploys or calls these agents: it lists them, and they may route their LLM
-- calls through the router. `discovered_on` names the machine the CLI reported them from
-- (NULL for every other agent) and is what the router and the registration endpoint key on.
-- Their rows keep the default `status = 'registered'` (or 'stopped' once the machine stops
-- reporting them), so the orchestrator and the A2A registry, which list only 'running'
-- agents, never route to them.
ALTER TABLE agents
    ADD COLUMN discovered_on TEXT;

-- One row per agent per machine: the same card name on two machines is two agents, and a
-- restart on a new port is the same one. `metadata->>'card_name'` is the card's `name` as
-- served; the row's own `name` is a slug that may carry the machine to stay unique.
CREATE UNIQUE INDEX agents_discovered_identity_uniq
    ON agents (owner_id, discovered_on, (metadata->>'card_name'))
    WHERE discovered_on IS NOT NULL AND deleted_at IS NULL;
