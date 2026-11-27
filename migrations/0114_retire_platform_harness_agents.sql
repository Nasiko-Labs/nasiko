-- Retire the platform's harness agents.
--
-- Each harness (`harness_catalog`) used to have one platform-owned `agents` row, named after the
-- harness (`name = harness_id`), that sessions were bound to. Every session is now bound to its
-- user's own harness agent (`harness-<harness id>`, `sessions::own_agent`), and users pick a
-- harness from the catalog, so the platform rows are no longer used.
--
-- 1. A user with a live session bound to a platform row gets their own harness agent for that
--    harness, as `sessions::own_agent::ensure` creates it (the display name and description match
--    `nasiko_runtime::harness_agent`). The image is the platform row's; the session's sandbox
--    takes its image from the catalog, not from this column. Approvals (`hitl_enabled`) are the
--    platform row's, as `ensure` copied them, so a session that asked before still asks.
-- 2. Each such session's sandbox token names the platform row, so it is revoked and forgotten; the
--    session's next turn mints one naming the user's agent.
-- 3. The session is bound to the user's agent. Its conversation is kept. Under the `pod` provider
--    its files lived under the platform row's folder and are not carried over.
-- 4. The platform rows are soft-deleted.
--
-- A session whose user already has an unrelated agent named `harness-<harness id>` stays bound to
-- the deleted platform row, and its turns then find no agent.

INSERT INTO agents (name, display_name, description, owner_id, image, status, is_public,
                    workload_kind, harness_id, parking_policy, writable, hitl_enabled)
SELECT DISTINCT ON (s.user_id, p.harness_id)
       'harness-' || p.harness_id,
       h.name || ' sessions',
       'Runs your ' || h.name || ' sessions with your own model settings and tools. '
           || 'Created by the platform and never shared.',
       s.user_id, p.image, 'parked', false, 'harness', p.harness_id, 'auto', true,
       p.hitl_enabled
FROM chat_sessions s
JOIN agents p ON p.id = s.agent_id AND p.workload_kind = 'harness' AND p.name = p.harness_id
JOIN harness_catalog h ON h.id = p.harness_id
WHERE s.deleted_at IS NULL
ORDER BY s.user_id, p.harness_id
ON CONFLICT (owner_id, name) WHERE deleted_at IS NULL DO NOTHING;

CREATE TEMPORARY TABLE moved_harness_sessions ON COMMIT DROP AS
SELECT s.session_id, s.sandbox_token_jti, own.id AS own_agent_id
FROM chat_sessions s
JOIN agents p ON p.id = s.agent_id AND p.workload_kind = 'harness' AND p.name = p.harness_id
JOIN agents own ON own.owner_id = s.user_id
               AND own.name = 'harness-' || p.harness_id
               AND own.workload_kind = 'harness'
               AND own.harness_id = p.harness_id
               AND own.deleted_at IS NULL
WHERE s.deleted_at IS NULL;

-- 30 days: SANDBOX_TOKEN_TTL_SECS, the longest a sandbox token lives.
INSERT INTO revoked_session_tokens (jti, expires_at)
SELECT sandbox_token_jti, now() + INTERVAL '30 days'
FROM moved_harness_sessions
WHERE sandbox_token_jti IS NOT NULL
ON CONFLICT (jti) DO NOTHING;

UPDATE chat_sessions s
   SET agent_id = m.own_agent_id, sandbox_token_jti = NULL, sandbox_token_enc = NULL
  FROM moved_harness_sessions m
 WHERE s.session_id = m.session_id;

UPDATE agents
   SET deleted_at = now(), is_public = false, updated_at = now()
 WHERE workload_kind = 'harness' AND name = harness_id AND deleted_at IS NULL;
