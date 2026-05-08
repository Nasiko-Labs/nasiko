"""
Gateway Key Manager
Mints, rotates, and deletes per-agent virtual keys on the LiteLLM gateway.

Virtual key lifecycle:
  - Minted at agent deploy time via POST /key/generate
  - Previous key for the same agent is deleted from Redis + gateway before minting
  - Keys are stored in Redis under agent:virtual_key:{agent_name} for rotation
"""

import logging
import httpx

logger = logging.getLogger(__name__)

REDIS_KEY_PREFIX = "agent:virtual_key:"


class GatewayKeyManager:
    def __init__(
        self,
        gateway_url: str,
        master_key: str,
        redis_client=None,
        default_max_budget: float | None = None,
    ):
        self.gateway_url = gateway_url.rstrip("/")
        self.master_key = master_key
        self.redis_client = redis_client
        # USD spend cap applied to every minted key unless overridden per-call.
        # None (the default) means unlimited — set via LLM_GATEWAY_DEFAULT_BUDGET env.
        self.default_max_budget = default_max_budget

    # ── Public API ────────────────────────────────────────────────────────────

    async def provision_key(
        self, agent_name: str, max_budget: float | None = None
    ) -> str | None:
        """
        Mint a fresh virtual key for agent_name.
        Deletes any previously provisioned key for this agent first (rotation).

        Args:
            agent_name: agent this key belongs to (used as LiteLLM key_alias).
            max_budget: USD spend cap for this key. Falls back to
                        ``default_max_budget`` if not provided.

        Returns:
            The new virtual key string, or None if the gateway is unavailable.
        """
        if not self.master_key:
            logger.warning(
                "LLM_GATEWAY_MASTER_KEY not set — skipping virtual key provisioning"
            )
            return None

        await self._rotate_existing_key(agent_name)
        budget = max_budget if max_budget is not None else self.default_max_budget
        return await self._mint_key(agent_name, max_budget=budget)

    async def revoke_key(self, agent_name: str) -> None:
        """Delete the virtual key for agent_name (called on agent teardown)."""
        await self._rotate_existing_key(agent_name)

    # ── Internal helpers ──────────────────────────────────────────────────────

    async def _mint_key(
        self, agent_name: str, max_budget: float | None = None
    ) -> str | None:
        payload = {
            "key_alias": f"nasiko-agent-{agent_name}",
            "metadata": {"agent_name": agent_name, "provisioned_by": "nasiko-orchestrator"},
            "duration": None,   # no expiry
            "max_budget": max_budget,   # None = unlimited
        }
        headers = {
            "Authorization": f"Bearer {self.master_key}",
            "Content-Type": "application/json",
        }
        try:
            async with httpx.AsyncClient(timeout=10) as client:
                resp = await client.post(
                    f"{self.gateway_url}/key/generate",
                    json=payload,
                    headers=headers,
                )
                resp.raise_for_status()
                key = resp.json().get("key")
                if key and self.redis_client:
                    self.redis_client.set(f"{REDIS_KEY_PREFIX}{agent_name}", key, ex=86400 * 30)
                logger.info(f"Minted virtual key for agent '{agent_name}'")
                return key
        except httpx.ConnectError:
            logger.warning(
                f"LLM gateway unreachable at {self.gateway_url} — "
                "agent will fall back to direct provider keys if configured"
            )
            return None
        except Exception as exc:
            logger.error(f"Failed to mint virtual key for '{agent_name}': {exc}")
            return None

    async def _rotate_existing_key(self, agent_name: str) -> None:
        """Delete the previously minted key for this agent from gateway + Redis."""
        if not self.redis_client:
            return
        redis_key = f"{REDIS_KEY_PREFIX}{agent_name}"
        existing = self.redis_client.get(redis_key)
        if not existing:
            return

        headers = {
            "Authorization": f"Bearer {self.master_key}",
            "Content-Type": "application/json",
        }
        try:
            async with httpx.AsyncClient(timeout=10) as client:
                resp = await client.post(
                    f"{self.gateway_url}/key/delete",
                    json={"keys": [existing]},
                    headers=headers,
                )
                if resp.status_code not in (200, 404):
                    logger.warning(
                        f"Unexpected status {resp.status_code} deleting old key for '{agent_name}'"
                    )
        except Exception as exc:
            logger.warning(f"Could not delete old gateway key for '{agent_name}': {exc}")
        finally:
            self.redis_client.delete(redis_key)
            logger.info(f"Rotated virtual key for agent '{agent_name}'")
