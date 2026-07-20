-- Per-agent LLM routing config, read by the LLM router to pick provider/model/key.
-- Shape (all fields optional except provider/model when the object is present):
--   { "provider": "anthropic", "model": "claude-3-5-sonnet-20241022",
--     "fallback_models": ["openai/gpt-4o-mini"], "temperature": 0.7,
--     "max_tokens": 2048, "api_key_secret_name": "ANTHROPIC_API_KEY" }
-- NULL (the default) → the router falls back to DEFAULT_PROVIDER / DEFAULT_MODEL and the
-- platform API key. Editable at runtime; the router caches reads for LLM_CONFIG_CACHE_TTL.
ALTER TABLE agents ADD COLUMN IF NOT EXISTS llm_config JSONB;
