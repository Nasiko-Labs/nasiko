"""HTTP forwarder used to call agents."""
import logging
import time
from dataclasses import dataclass

import httpx

logger = logging.getLogger(__name__)


@dataclass
class ForwardResult:
    status_code: int
    headers: dict[str, str]
    body: bytes
    latency_ms: float


class Forwarder:
    """A long-lived async HTTP client used to talk to agents.

    The client is constructed once at FastAPI startup and shared across
    requests (httpx pools connections internally).
    """

    def __init__(
        self,
        *,
        timeout_seconds: float = 30.0,
        max_connections: int = 100,
        max_keepalive_connections: int = 50,
    ) -> None:
        limits = httpx.Limits(
            max_connections=max_connections,
            max_keepalive_connections=max_keepalive_connections,
        )
        self._client = httpx.AsyncClient(
            timeout=httpx.Timeout(timeout_seconds),
            limits=limits,
            follow_redirects=False,
        )

    async def close(self) -> None:
        await self._client.aclose()

    async def forward(
        self,
        *,
        method: str,
        url: str,
        headers: dict[str, str],
        body: bytes,
        params: dict[str, str] | list[tuple[str, str]] | None = None,
    ) -> ForwardResult:
        """Forward an HTTP request to ``url``.

        Returns a :class:`ForwardResult` containing status, headers, body,
        and round-trip latency. Raises ``httpx.HTTPError`` only on transport
        failures; HTTP-level non-2xx responses are returned, not raised.
        """

        cleaned = _strip_hop_by_hop(headers)
        start = time.perf_counter()
        response = await self._client.request(
            method=method,
            url=url,
            content=body if body else None,
            headers=cleaned,
            params=params,
        )
        latency_ms = (time.perf_counter() - start) * 1000.0
        return ForwardResult(
            status_code=response.status_code,
            headers=dict(response.headers),
            body=response.content,
            latency_ms=latency_ms,
        )


_HOP_BY_HOP = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
}


def _strip_hop_by_hop(headers: dict[str, str]) -> dict[str, str]:
    return {k: v for k, v in headers.items() if k.lower() not in _HOP_BY_HOP}
