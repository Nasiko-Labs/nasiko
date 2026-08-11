-- Per-agent inbound SDK format (Phase 2, P2.5).
--
-- Which LLM SDK the agent's *code* speaks, so the deploy-time injector writes the
-- matching base-URL/key env vars (OpenAI → OPENAI_*, Anthropic → ANTHROPIC_*, Gemini →
-- GOOGLE_*/GEMINI_*). Independent of the *outbound* provider in agents.llm_config (where
-- we route the call). Defaults to 'openai' — backward compatible for existing agents.
ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS inbound_format TEXT NOT NULL DEFAULT 'openai'
    CHECK (inbound_format IN ('openai', 'anthropic', 'gemini'));
