"""MCP Gateway client helper for Nasiko agents.

Every deployed agent gets one env var at deploy time — ``MCP_GATEWAY_URL`` —
pointing at the platform's aggregating MCP gateway (e.g.
``http://gateway:8080/api/mcp``). It exposes *all* tools the user has connected
(Gmail, SerpAPI, Notion, …), merged and permission-filtered, behind one URL.

Identity is per-request: when the edge gateway invokes your agent on behalf of a
user, it injects a short-lived delegation token in the inbound
``X-Nasiko-Agent-Token`` header. **The one contract every agent must honor:**
read that inbound header and forward it here. The gateway validates it and scopes
every tool call to *(that user, this agent)* — you never handle user credentials.

Usage (framework-agnostic — you supply the inbound token):

    from common.mcp_gateway import MCPGatewayClient, AGENT_TOKEN_HEADER

    # 1. Capture the inbound delegation token from your web framework's request.
    #    (Starlette/A2A: `request.headers.get(AGENT_TOKEN_HEADER)`.)
    token = incoming_headers.get(AGENT_TOKEN_HEADER)

    # 2. Use the gateway for the duration of this request.
    async with MCPGatewayClient.from_env(token) as mcp:
        tools = await mcp.list_tools()
        result = await mcp.call_tool("serpapi__search", {"params": {"q": "..."}})

If ``MCP_GATEWAY_URL`` is unset (local dev without the gateway), ``from_env``
returns ``None`` so callers can degrade gracefully.
"""
from __future__ import annotations

import json
import os
from typing import Any

import httpx

#: Inbound header carrying the gateway-minted delegation token.
AGENT_TOKEN_HEADER = "X-Nasiko-Agent-Token"

_PROTOCOL_VERSION = "2024-11-05"


class MCPGatewayError(RuntimeError):
    """A JSON-RPC error returned by the MCP gateway (code + message)."""

    def __init__(self, code: int, message: str):
        super().__init__(f"MCP gateway error {code}: {message}")
        self.code = code
        self.message = message


class MCPGatewayClient:
    """Thin async MCP JSON-RPC client for the Nasiko gateway.

    Speaks the standard MCP streamable-HTTP transport (handles both
    ``application/json`` and ``text/event-stream`` responses) and forwards the
    per-request delegation token on every call.
    """

    def __init__(
        self,
        gateway_url: str,
        agent_token: str | None,
        *,
        traceparent: str | None = None,
        timeout: float = 60.0,
    ):
        self._url = gateway_url
        self._token = agent_token
        # Forwarding the inbound W3C `traceparent` lets the gateway attribute tool
        # calls to the chat's flow — required for the phase-2 approval (HITL) event
        # to reach the right chat stream.
        self._traceparent = traceparent
        self._client = httpx.AsyncClient(timeout=timeout)
        self._id = 0

    @classmethod
    def from_env(cls, agent_token: str | None, **kwargs: Any) -> "MCPGatewayClient | None":
        """Build a client from ``MCP_GATEWAY_URL``; ``None`` if it is unset."""
        url = os.environ.get("MCP_GATEWAY_URL")
        if not url:
            return None
        return cls(url, agent_token, **kwargs)

    async def __aenter__(self) -> "MCPGatewayClient":
        return self

    async def __aexit__(self, *_exc: Any) -> None:
        await self.aclose()

    async def aclose(self) -> None:
        await self._client.aclose()

    # ── MCP methods ─────────────────────────────────────────────────────────

    async def initialize(self) -> dict:
        """Perform the MCP capability handshake."""
        return await self._rpc("initialize", None)

    async def list_tools(self) -> list[dict]:
        """Return the merged, permission-filtered tool list for this user+agent."""
        result = await self._rpc("tools/list", {})
        return result.get("tools", [])

    async def call_tool(self, name: str, arguments: dict | None = None) -> dict:
        """Invoke a tool by its (namespaced) name; returns the backend result.

        Raises :class:`MCPGatewayError` if the tool is blocked (-32000) or needs
        approval (-32001) for this agent.
        """
        return await self._rpc("tools/call", {"name": name, "arguments": arguments or {}})

    # ── transport ─────────────────────────────────────────────────────────────

    async def _rpc(self, method: str, params: dict | None) -> dict:
        self._id += 1
        body: dict[str, Any] = {"jsonrpc": "2.0", "id": self._id, "method": method}
        if params is not None:
            body["params"] = params

        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        }
        if self._token:
            headers[AGENT_TOKEN_HEADER] = self._token
        if self._traceparent:
            headers["traceparent"] = self._traceparent

        async with self._client.stream("POST", self._url, json=body, headers=headers) as resp:
            resp.raise_for_status()
            content_type = resp.headers.get("content-type", "")
            if "text/event-stream" in content_type:
                payload = await _read_first_sse_data(resp)
            else:
                payload = json.loads(await resp.aread())

        if "error" in payload and payload["error"] is not None:
            err = payload["error"]
            raise MCPGatewayError(err.get("code", -32603), err.get("message", "unknown error"))
        return payload.get("result", {})

    @property
    def protocol_version(self) -> str:
        return _PROTOCOL_VERSION


async def _read_first_sse_data(resp: httpx.Response) -> dict:
    """Read SSE frames until the first non-empty ``data:`` JSON payload."""
    async for raw in resp.aiter_lines():
        line = raw.strip()
        if line.startswith("data:"):
            data = line[5:].strip()
            if data and data != "[DONE]":
                return json.loads(data)
    raise MCPGatewayError(-32603, "empty event-stream from MCP gateway")
