-- When the owner last revoked this agent's LLM routing tokens (`DELETE /api/agents/{id}/llm-token`,
-- called by `nasiko disconnect <coding-agent>`). The LLM router refuses any agent token issued
-- before it, so a coding-agent session still holding a cached token stops spending through the
-- router the moment the user disconnects instead of when the token expires. NULL = never revoked.
-- Written from the control plane's clock, the same clock that stamps a token's `iat`.
ALTER TABLE agents ADD COLUMN IF NOT EXISTS llm_tokens_revoked_at TIMESTAMPTZ;
