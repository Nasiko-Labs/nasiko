"""A one-tool MCP server for the e2e: `add`, over streamable HTTP at /mcp.

Usage: python mock_mcp.py <port>
"""

import sys

from mcp.server.fastmcp import FastMCP

server = FastMCP("e2e-calc", host="127.0.0.1", port=int(sys.argv[1]), stateless_http=True, json_response=True)


@server.tool()
def add(a: int, b: int) -> int:
    """Add two integers."""
    return a + b


server.run(transport="streamable-http")
