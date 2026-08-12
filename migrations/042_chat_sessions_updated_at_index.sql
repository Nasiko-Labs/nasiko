-- Index the sessions list's actual sort order.
--
-- `GET /api/chat/sessions` orders by (updated_at DESC, session_id DESC) and
-- pages with a keyset cursor on that same pair, but the only user-scoped index
-- was `idx_chat_sessions_recent (user_id, created_at DESC)` — the wrong column.
-- Without a matching index Postgres cannot stop early: it reads every session
-- the user owns, evaluates the list query's LATERAL joins for each one, and
-- only then sorts and applies LIMIT. The per-session aggregate (message and
-- trace counts, billed tokens, p50 latency) scans that session's whole message
-- history, so the cost scaled with the user's total session count rather than
-- with the page size.
--
-- Measured on 5000 sessions x 40 messages: 182ms and `loops=5000` on the
-- aggregate before, 0.75ms and `loops=25` after.
--
-- Column order matters: user_id equality first, then updated_at DESC to satisfy
-- the ORDER BY, then session_id DESC as the keyset tiebreaker.
CREATE INDEX IF NOT EXISTS idx_chat_sessions_user_updated
    ON chat_sessions (user_id, updated_at DESC, session_id DESC);
