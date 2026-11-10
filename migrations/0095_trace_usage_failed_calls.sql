-- =============================================================================
-- trace_usage: failed LLM calls the router served (NAS-1018).
--
-- A provider call that failed (a 5xx/429, a timeout, an error mid-stream) used to
-- leave no trace in spend at all: the router returned the error before writing its
-- row, and `error_count` (0059) stayed 0. The provider may still bill such a call,
-- so cost reports came out low and failing tasks looked healthy.
--
-- The router now adds each failed call to its (trace_id, agent_name) row like any
-- other call: tokens the provider reported go into the spend columns and are
-- priced; tokens it did not report are estimated from the request size and kept
-- out of them (never priced, never billed), and the row is flagged
-- `cost_estimated`, because its cost may be understated.
--
-- cold: compatible (hot-only columns, like the router_* share in 0079: the cold copy
-- and the rollups carry the visible `error_count`, which these feed)
-- =============================================================================

ALTER TABLE trace_usage
    -- The router's share of `error_count`, kept apart for the same reason the
    -- router_* spend columns are: the Tempo materializer sets `error_count` from
    -- the spans it sees, so it writes GREATEST(tempo, router_error_count) and a
    -- failure the router booked live is never erased by a later scan.
    ADD COLUMN router_error_count INTEGER NOT NULL DEFAULT 0,
    -- Prompt tokens of failed calls whose provider reported no usage, estimated from
    -- the bytes sent. An estimate, so never added to `input_tokens` or priced.
    ADD COLUMN router_estimated_input_tokens BIGINT NOT NULL DEFAULT 0;
