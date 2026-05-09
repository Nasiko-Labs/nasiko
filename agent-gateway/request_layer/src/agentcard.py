"""Capability adapter for Nasiko AgentCards."""
import asyncio
import logging
from typing import Any, Iterable

import httpx

from request_layer.src.cache.policy import infer_policy
from request_layer.src.config import Settings
from request_layer.src.types import AgentManifest, Policy

logger = logging.getLogger(__name__)


class NasikoAdapter:
    """Discovers Nasiko agents via the backend registry endpoint.

    Constructed once at FastAPI startup and refreshed in a background loop
    every ``request_layer_registry_poll_seconds`` (default 60). The latest snapshot
    is exposed via :attr:`manifests` and :attr:`policies`.
    """

    def __init__(self, settings: Settings) -> None:
        self._settings = settings
        self._client = httpx.AsyncClient(timeout=httpx.Timeout(10.0))
        self._manifests: dict[str, AgentManifest] = {}
        self._policies: dict[str, Policy] = {}
        self._lock = asyncio.Lock()

    @property
    def manifests(self) -> dict[str, AgentManifest]:
        return dict(self._manifests)

    @property
    def policies(self) -> dict[str, Policy]:
        return dict(self._policies)

    async def close(self) -> None:
        await self._client.aclose()

    async def refresh(self) -> None:
        """Re-pull the registry. Logs and swallows transient errors."""

        try:
            cards = await self._fetch_cards()
        except Exception:  # noqa: BLE001 — adapter must not crash poll loop
            logger.exception("failed to refresh Nasiko registry")
            return

        manifests: dict[str, AgentManifest] = {}
        policies: dict[str, Policy] = {}
        for card in cards:
            manifest = parse_agentcard(card)
            if manifest is None:
                continue
            manifests[manifest.name] = manifest
            policies[manifest.name] = infer_policy(manifest, self._settings)

        async with self._lock:
            self._manifests = manifests
            self._policies = policies

        logger.info("registry refreshed: %s agents", len(manifests))

    async def _fetch_cards(self) -> list[dict[str, Any]]:
        """Pull every AgentCard exposed by the Nasiko backend.

        The backend exposes registry queries via
        ``/api/v1/registry/agents`` (paginated). This helper handles the
        case where the endpoint returns either a list directly or an
        envelope with a ``data`` key.
        """

        base = self._settings.request_layer_nasiko_registry_url.rstrip("/")
        url = f"{base}/api/v1/registry/agents"
        response = await self._client.get(url)
        response.raise_for_status()
        payload = response.json()
        if isinstance(payload, list):
            return payload
        if isinstance(payload, dict):
            data = payload.get("data")
            if isinstance(data, list):
                return data
            if isinstance(data, dict):
                # Some endpoints wrap a single agent — coerce to list.
                return [data]
        return []

    async def poll_loop(self) -> None:
        """Background coroutine that refreshes the snapshot periodically."""

        interval = max(5, self._settings.request_layer_registry_poll_seconds)
        while True:
            await self.refresh()
            await asyncio.sleep(interval)


def parse_agentcard(card: dict[str, Any]) -> AgentManifest | None:
    """Normalize a Nasiko AgentCard into an :class:`AgentManifest`.

    Returns ``None`` if the card lacks the minimum fields (a name and a
    URL).
    """

    name = card.get("name") or card.get("id")
    url = card.get("url") or card.get("agent_url")
    if not name or not url:
        return None

    capabilities: set[str] = set()
    tags: set[str] = set()
    examples: list[str] = []

    raw_caps = card.get("capabilities")
    if isinstance(raw_caps, dict):
        capabilities |= {k for k, v in raw_caps.items() if v}
    elif isinstance(raw_caps, list):
        capabilities |= {str(c) for c in raw_caps}

    skills = card.get("skills") or []
    if isinstance(skills, list):
        for skill in skills:
            if not isinstance(skill, dict):
                continue
            if skill.get("id"):
                capabilities.add(str(skill["id"]).lower())
            for tag in _ensure_iterable(skill.get("tags")):
                tags.add(str(tag).lower())
            for example in _ensure_iterable(skill.get("examples")):
                if isinstance(example, str):
                    examples.append(example)

    for tag in _ensure_iterable(card.get("tags")):
        tags.add(str(tag).lower())

    model = card.get("model")
    provider = card.get("provider")
    if not model and isinstance(provider, dict):
        model = provider.get("model")

    return AgentManifest(
        name=str(name),
        endpoint_url=str(url),
        capabilities=capabilities,
        tags=tags,
        examples=examples,
        model=str(model) if model else None,
        raw=card,
    )


def _ensure_iterable(value: Any) -> Iterable[Any]:
    if value is None:
        return ()
    if isinstance(value, (list, tuple, set)):
        return value
    return (value,)
