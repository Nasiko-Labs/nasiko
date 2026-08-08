-- Per-message usage + trace linkage for chat history.
--
-- Populated for platform-paid usage only (the LLM router / orchestrator);
-- bring-your-own-key agent spend is deliberately not metered — those
-- messages carry duration/trace only. `usage_estimated` marks token counts
-- derived from a character estimate (streamed orchestrator turns, where the
-- provider stream reports no usage) so UIs can label them approximate.
ALTER TABLE chat_messages
    ADD COLUMN IF NOT EXISTS input_tokens INTEGER,
    ADD COLUMN IF NOT EXISTS output_tokens INTEGER,
    ADD COLUMN IF NOT EXISTS model TEXT,
    ADD COLUMN IF NOT EXISTS duration_ms INTEGER,
    ADD COLUMN IF NOT EXISTS cost_usd DECIMAL(10, 8),
    ADD COLUMN IF NOT EXISTS usage_estimated BOOLEAN,
    ADD COLUMN IF NOT EXISTS trace_id TEXT;
