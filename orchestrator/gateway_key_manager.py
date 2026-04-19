"""
GatewayKeyManager — per-agent LiteLLM virtual key provisioning.

MVP: master-key mode (shared virtual-key injected into every agent).
Target: per-agent key via POST /key/generate, stored in Redis, rotated
        on every deploy and revoked via /key/delete on teardown.

Switching between modes requires changing only this module — no agent
code changes, no compose changes.
"""

import logging
import requests
import redis
from config import Config

logger = logging.getLogger(__name__)

REDIS_KEY_PREFIX = "nasiko:litellm:keys"


class GatewayKeyManager:
    """Mints, stores, and revokes per-agent LiteLLM virtual keys."""

    def __init__(self):
        self._base_url = Config.LITELLM_URL.rstrip("/")
        self._master_key = Config.LITELLM_MASTER_KEY
        self._budget = Config.LITELLM_AGENT_BUDGET_USD
        self._ttl = Config.LITELLM_KEY_TTL_SECONDS
        self._default_model = Config.LITELLM_DEFAULT_MODEL
        self._headers = {
            "Authorization": f"Bearer {self._master_key}",
            "Content-Type": "application/json",
        }
        # Redis client — same connection the rest of the orchestrator uses
        self._redis = redis.Redis(
            host=Config.REDIS_HOST,
            port=Config.REDIS_PORT,
            db=Config.REDIS_DB,
            decode_responses=True,
        )

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def mint_key_for_agent(self, agent_name: str, owner_id: str = None) -> str:
        """
        Provision a per-agent virtual key from the LiteLLM gateway.

        Returns the minted key string. Falls back to master-key if the
        gateway is unreachable so deployments never block on gateway state.
        """
        # Revoke any existing key for this agent before minting a fresh one
        self._revoke_existing_key(agent_name)

        try:
            payload = {
                "key_alias": f"nasiko-agent-{agent_name}",
                "models": [
                    self._default_model,
                    "llama3",
                    "llama3-fast",
                    "gpt-4o-mini",
                    "gpt-4o",
                ],
                "max_budget": self._budget,
                "duration": f"{self._ttl}s",
                "metadata": {
                    "agent_name": agent_name,
                    "owner_id": str(owner_id) if owner_id else "platform",
                    "provisioned_by": "nasiko-orchestrator",
                },
            }
            response = requests.post(
                f"{self._base_url}/key/generate",
                json=payload,
                headers=self._headers,
                timeout=5,
            )
            response.raise_for_status()
            key = response.json()["key"]

            # Persist in Redis so we can revoke later
            redis_key = f"{REDIS_KEY_PREFIX}:{agent_name}"
            self._redis.set(redis_key, key, ex=self._ttl)

            logger.info(
                f"Minted per-agent key for '{agent_name}' (budget: ${self._budget})"
            )
            return key

        except Exception as e:
            # Gateway unavailable or key/generate not supported — fall back to master key.
            # This keeps deployments working even if LiteLLM is still starting up.
            logger.warning(
                f"Could not mint per-agent key for '{agent_name}': {e}. "
                "Falling back to shared virtual-key."
            )
            return self._master_key

    def revoke_key_for_agent(self, agent_name: str) -> bool:
        """Revoke the agent's virtual key from the gateway and remove from Redis."""
        return self._revoke_existing_key(agent_name)

    def get_key_for_agent(self, agent_name: str) -> str | None:
        """Look up the current key for an agent from Redis."""
        redis_key = f"{REDIS_KEY_PREFIX}:{agent_name}"
        return self._redis.get(redis_key)

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    def _revoke_existing_key(self, agent_name: str) -> bool:
        redis_key = f"{REDIS_KEY_PREFIX}:{agent_name}"
        existing_key = self._redis.get(redis_key)
        if not existing_key:
            return False
        try:
            response = requests.post(
                f"{self._base_url}/key/delete",
                json={"keys": [existing_key]},
                headers=self._headers,
                timeout=5,
            )
            response.raise_for_status()
            self._redis.delete(redis_key)
            logger.info(f"Revoked key for agent '{agent_name}'")
            return True
        except Exception as e:
            logger.warning(f"Could not revoke key for '{agent_name}': {e}")
            self._redis.delete(
                redis_key
            )  # Remove from Redis even if gateway call fails
            return False
