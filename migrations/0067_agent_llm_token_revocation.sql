-- When an agent's owner last revoked its LLM keys (`DELETE /api/agents/{id}/llm-token`, which
-- `nasiko disconnect` calls for a discovered agent). The keys are signed tokens the router
-- verifies without a lookup, so a revocation is a cut-off: the router refuses every key minted
-- (`iat`) up to this time. NULL = never revoked.
ALTER TABLE agents
    ADD COLUMN llm_tokens_revoked_at TIMESTAMPTZ;
