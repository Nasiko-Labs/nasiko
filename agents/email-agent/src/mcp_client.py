"""Generic client for the Nasiko MCP Gateway (oss/docs/MCP_GATEWAY_DESIGN.md):
`tools/list` (tool discovery) and `tools/call` (invocation), used by
agent_executor.py's LLM tool loop. Calls whatever tool name/arguments the LLM
decides to use — the gateway's tool catalog is discovered at runtime.
"""

import uuid

import httpx
from opentelemetry.context import attach, detach, set_value
from opentelemetry.instrumentation.utils import _SUPPRESS_INSTRUMENTATION_KEY

# Error codes the gateway returns for a blocked/ask-gated/auth-broken tool
# (oss/mcp-gateway/src/types.rs::codes).
TOOL_BLOCKED = -32000
TOOL_ASK = -32001
AUTH_REQUIRED = -32002


class McpError(Exception):
    """kind is one of "denied", "ask_required", "auth_required", "other"."""

    def __init__(self, kind: str, message: str, data: dict | None = None):
        self.kind = kind
        self.message = message
        self.data = data or {}
        super().__init__(f"{kind}: {message}")


async def _call(
    client: httpx.AsyncClient,
    gateway_url: str,
    token: str,
    method: str,
    params: dict | None,
    traceparent: str | None = None,
) -> dict:
    request = {"jsonrpc": "2.0", "id": str(uuid.uuid4()), "method": method}
    if params is not None:
        request["params"] = params

    headers = {"Authorization": f"Bearer {token}"}
    # The gateway authorizes tools/call by checking this agent is a recorded
    # flow_participants member of the flow this trace id names
    # (docs/MCP_GATEWAY_AGENT_AUTH.md) — without it every call is rejected 403
    # regardless of token validity.
    if traceparent:
        headers["traceparent"] = traceparent

    # OTel's HTTPXClientInstrumentor (telemetry.py) unconditionally overwrites any
    # `traceparent` header already on the request with one derived from whatever
    # ambient span context exists for the current task — which, for MCP calls
    # made deep in the LLM tool loop, is never the real inbound flow's span, so
    # it silently replaced the real traceparent above with a fresh, never-
    # registered one on every call (confirmed live: tools/call 403'd with
    # "does not resolve to a live flow" for a trace_id that never appeared
    # anywhere else, while the identical header worked fine sent manually
    # outside the instrumented client). Suppressing instrumentation for just
    # this call is the standard OTel mechanism for "don't let auto-
    # instrumentation touch this request" — it stops the override without
    # disabling propagation for the rest of the process.
    token_ctx = attach(set_value(_SUPPRESS_INSTRUMENTATION_KEY, True))
    try:
        resp = await client.post(gateway_url, json=request, headers=headers)
    except httpx.HTTPError as e:
        raise McpError("other", f"HTTP error calling MCP gateway: {e}") from e
    finally:
        detach(token_ctx)

    try:
        body = resp.json()
    except ValueError as e:
        raise McpError("other", f"MCP gateway {resp.status_code}: invalid JSON response: {e}") from e

    if "error" in body:
        error = body["error"]
        code = error.get("code", 0)
        message = error.get("message", "unknown error")
        data = error.get("data") or {}
        if code == TOOL_ASK:
            raise McpError("ask_required", message, data)
        if code == AUTH_REQUIRED:
            raise McpError("auth_required", message, data)
        if code == TOOL_BLOCKED:
            raise McpError("denied", message, data)
        raise McpError("other", f"MCP error {code}: {message}")

    if "result" not in body:
        raise McpError("other", "MCP response had neither result nor error")

    return body["result"]


async def list_tools(
    client: httpx.AsyncClient, gateway_url: str, token: str, traceparent: str | None = None
) -> list[dict]:
    result = await _call(client, gateway_url, token, "tools/list", None, traceparent)
    return result.get("tools", [])


async def call_tool(
    client: httpx.AsyncClient,
    gateway_url: str,
    token: str,
    name: str,
    arguments: dict,
    traceparent: str | None = None,
) -> dict:
    return await _call(
        client, gateway_url, token, "tools/call", {"name": name, "arguments": arguments}, traceparent
    )
