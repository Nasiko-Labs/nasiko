"""Minimal MCP server for integration tests: one no-op `echo` tool, streamable
HTTP transport mounted at /mcp, listening on $PORT (the platform's convention
for uploaded MCP servers — see docs/MCP_UPLOAD_PLAN_OSS.md §5.2)."""

import os

from mcp.server.fastmcp import FastMCP

PORT = int(os.environ.get("PORT", "8080"))

mcp = FastMCP(
    "echo-server",
    host="0.0.0.0",
    port=PORT,
    streamable_http_path="/mcp",
)


@mcp.tool()
def echo(message: str) -> str:
    """Echo the input message back unchanged."""
    return message


if __name__ == "__main__":
    mcp.run(transport="streamable-http")
