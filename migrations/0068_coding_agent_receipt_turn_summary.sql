-- A typed summary of each coding-agent receipt's turn, so `session/list` can total a coding
-- session (turns, tokens, duration, tool failures) without reading `payload`. The payload carries
-- the prompt, the response and every tool call's output; reaching into it detoasts all of that for
-- every turn on the page, where these four columns cost a few bytes each.
--
-- Written by ingest alongside the payload they summarize. Nullable rather than NOT NULL so a
-- receipt written by an older binary during a rolling deploy still lands; the list treats a NULL
-- as "not summarized" (the turn still counts, its tokens and times do not).
--
-- Schema only. Receipts that predate these columns are summarized by the server in bounded chunks
-- after startup (`coding_agent_turn_summary_backfill.rs`), never by one table-wide UPDATE here:
-- that would rewrite every receipt's row inside the deploy's migration transaction and hold its
-- lock on the whole table until the last one was done. Nothing here takes more than a brief
-- catalog lock, since nullable columns with no default are metadata-only additions.
--
-- `IF NOT EXISTS` so a development database that applied an earlier draft of this migration (which
-- carried the backfill inline) can drop its `_sqlx_migrations` row for version 63 and re-run it.
ALTER TABLE coding_agent_telemetry_events
    ADD COLUMN IF NOT EXISTS turn_started_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS turn_ended_at TIMESTAMPTZ,
    -- input + output + cache read + cache creation, over every LLM call in the turn.
    ADD COLUMN IF NOT EXISTS turn_tokens BIGINT,
    -- Tool calls that ended in an error ('failed' or 'timed_out'). Denied and cancelled calls are
    -- the user's decision, not a failure.
    ADD COLUMN IF NOT EXISTS turn_tool_errors INTEGER;
