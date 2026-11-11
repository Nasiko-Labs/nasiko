-- Adds the two tool-list layers to the savings ledger, both under the `tool_lists` program.
--
-- `compact_tools`: one row per llm-router call sent with compact tool signatures instead of the
-- native schemas, whose reply decoded (a call re-sent natively saved nothing).
-- `tool_search`: one row per MCP gateway `tools/list` served from search, against the generic
-- connector tools the listing would have held with search off. Signed: the search meta-tools are
-- listed too, so a small catalog can come out negative.
-- `context_budget` (its own program): context an edition injects into an agent's prompt, packed
-- under a token budget. Nothing in OSS writes it yet; it is admitted here so its writer needs no
-- migration of its own.
--
-- 0052 declared the check inline, so Postgres named it `token_savings_layer_check`. NOT VALID so
-- adding the constraint does not scan the table under this migration's exclusive lock, which would
-- hold up the router's savings inserts; 0100 validates it under a lock that does not block reads or
-- writes. The new set is a superset of the old one, so validation cannot fail.
--
-- cold: compatible (the check governs hot rows only; tiering copies `layer` as a plain string, so
-- cold rows need no rewrite)
ALTER TABLE token_savings
    DROP CONSTRAINT IF EXISTS token_savings_layer_check,
    ADD CONSTRAINT token_savings_layer_check CHECK (layer IN (
        'compress_payload',
        'compress_tool_result',
        'compress_history',
        'context_selection',
        'brevity',
        'minimal_code',
        'prompt_comments',
        'compact_tools',
        'tool_search',
        'context_budget')) NOT VALID;
