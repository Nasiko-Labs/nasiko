-- Pricing for the models the LLM router advertises (GET /v1/models) that migration
-- 002 didn't already seed. The token_usage cost trigger reads model_pricing by
-- (provider, model) and fills cost_usd; without a row, cost is left NULL (best-effort,
-- per spec). 002 already prices openai gpt-4o / gpt-4o-mini.
--
-- Prices are USD per 1M tokens, best-effort public list rates as of authoring —
-- VERIFY against current provider pricing before relying on cost figures.
-- Columns match 002's seed: (provider, model, input/output per-1M, cache create/read, notes).
INSERT INTO model_pricing
    (provider, model, input_price_per_1m, output_price_per_1m, cache_creation_price_per_1m, cache_read_price_per_1m, notes)
VALUES
    -- Anthropic (the journey/catalog model + a cheaper sibling). Cache: 1.25x write, 0.1x read.
    ('anthropic', 'claude-3-5-sonnet-20241022', 3.00, 15.00, 3.75, 0.30, 'Claude 3.5 Sonnet'),
    ('anthropic', 'claude-3-5-haiku-20241022',  0.80,  4.00, 1.00, 0.08, 'Claude 3.5 Haiku'),
    -- Gemini (no prompt-cache pricing modeled here).
    ('gemini', 'gemini-1.5-pro',   1.25, 5.00, NULL, NULL, 'Gemini 1.5 Pro'),
    ('gemini', 'gemini-1.5-flash', 0.075, 0.30, NULL, NULL, 'Gemini 1.5 Flash'),
    ('gemini', 'gemini-2.0-flash', 0.10, 0.40, NULL, NULL, 'Gemini 2.0 Flash'),
    -- OpenAI embeddings (no output tokens).
    ('openai', 'text-embedding-3-small', 0.02, 0.00, NULL, NULL, 'OpenAI text-embedding-3-small'),
    ('openai', 'text-embedding-3-large', 0.13, 0.00, NULL, NULL, 'OpenAI text-embedding-3-large');
