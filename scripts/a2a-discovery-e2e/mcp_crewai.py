"""Run the MCP snippet `nasiko connect` prints for a CrewAI agent, with the variables it wrote
to the agent's env file, and reach the e2e's `add` tool the way the agent's model would:
outside a Nasiko flow the gateway lists its search and call tools, so find `add`, then call it.

Usage: python mcp_crewai.py <env file>
"""

import json
import os
import sys

from env_file import load, text

load(sys.argv[1])

from crewai import Agent

# ── as printed by `nasiko connect` ──
from crewai.mcp import MCPServerHTTP

nasiko = MCPServerHTTP(
    url=os.environ["MCP_GATEWAY_URL"],
    headers={"Authorization": f"Bearer {os.environ['MCP_GATEWAY_TOKEN']}"},
)
# ──

agent = Agent(role="calculator", goal="add numbers", backstory="e2e", mcps=[nasiko])
tools = {tool.name: tool for tool in agent.get_mcp_tools(agent.mcps)}
search = next(tool for name, tool in tools.items() if name.endswith("nasiko_search_tools"))
call = next(tool for name, tool in tools.items() if name.endswith("nasiko_call_tool"))
found = json.loads(text(search.run(query="add two integers")))
names = [tool["name"] for tool in found["tools"]]
add = next((name for name in names if name.endswith("add")), None)
if add is None:
    sys.exit(f"the search found no add tool: {names}")
print(text(call.run(name=add, arguments={"a": 2, "b": 3})))
