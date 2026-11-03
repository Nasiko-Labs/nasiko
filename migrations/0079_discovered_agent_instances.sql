-- Two copies of one A2A agent on one machine (the same card name, served from two folders or
-- containers) are two agents. The CLI reports which copy each is as `metadata->>'instance'`:
-- an opaque hash of the folder or container serving it, stable across restarts and port
-- changes. Rows registered before it carry none and are claimed by the first copy reported
-- with one, so they keep their id, key and history.
DROP INDEX agents_discovered_identity_uniq;

CREATE UNIQUE INDEX agents_discovered_identity_uniq
    ON agents (owner_id, discovered_on, (metadata->>'card_name'), COALESCE(metadata->>'instance', ''))
    WHERE discovered_on IS NOT NULL AND deleted_at IS NULL;
