"""Run the MCP snippet `nasiko connect` prints for a LangGraph agent, with the variables it
wrote to the agent's env file, and reach the e2e's `add` tool the way the agent's model would:
outside a Nasiko flow the gateway lists its search and call tools, so find `add`, then call it.

Usage: python mcp_langgraph.py <env file>
"""

import asyncio
import json
import os
import sys

from env_file import load, text

load(sys.argv[1])

# ── as printed by `nasiko connect` ──
from langchain_mcp_adapters.client import MultiServerMCPClient

mcp = MultiServerMCPClient({"nasiko": {
    "transport": "streamable_http",
    "url": os.environ["MCP_GATEWAY_URL"],
    "headers": {"Authorization": f"Bearer {os.environ['MCP_GATEWAY_TOKEN']}"},
}})
# ──


async def main() -> None:
    tools = {tool.name: tool for tool in await mcp.get_tools()}
    found = json.loads(text(await tools["nasiko_search_tools"].ainvoke({"query": "add two integers"})))
    names = [tool["name"] for tool in found["tools"]]
    add = next((name for name in names if name.endswith("add")), None)
    if add is None:
        sys.exit(f"the search found no add tool: {names}")
    print(text(await tools["nasiko_call_tool"].ainvoke({"name": add, "arguments": {"a": 2, "b": 3}})))


asyncio.run(main())
