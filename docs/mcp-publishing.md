# Publishing MCP Servers to Nasiko

The Nasiko platform supports running arbitrary Model Context Protocol (MCP) servers locally as first-class citizens, identically to autonomous agents! What this means is that when you write a custom MCP server utilizing standard SDKs (like `@mcp/sdk` or Python's `mcp.server.FastMCP`), you can Zip it up and deploy it straight to the Nasiko Hub.

## How it works

1. **Upload your code** - Simply upload a `.zip` file of your MCP server. Nasiko statically parses `src/main.py` or `main.py` and auto-detects MCP initialization keywords!
2. **Auto-Schematization** - The orchestration engine transparently analyzes the tools and prompts declared in your server and creates an automated Capabilities Manifest (`McpServerManifest.json`).
3. **HTTP Bridge** - Behind the scenes, the Nasiko cluster will start your MCP server via `stdio` and wrap it in an asynchronous HTTP Bridge (`mcp_bridge.py`), serving requests safely to the internal cluster network while maintaining standard stdio I/O.
4. **Tool Wiring** - Autonomous AI Agents deployed on Nasiko can simply pass the URL of your MCP container within their `MCP_SERVERS` environment variable to instantly auto-import every Tool!

## Example FastMCP Server

```python
# main.py
from mcp.server.fastmcp import FastMCP
from typing import Literal

mcp = FastMCP("weather-mcp")

@mcp.tool()
def get_weather(city: str) -> str:
    """Returns the weather for a given city."""
    if city.lower() == "london": return "Rainy"
    return "Sunny"

if __name__ == "__main__":
    mcp.run()
```

## Consuming your MCP Server inside an Agent

Once your MCP Server is deployed, it receives an internal DNS record exactly like Agents do:
`http://kong-gateway:8000/agents/agent-[mcp-name]`

In your Agent environment configuration (`.env`):
```bash
MCP_SERVERS=http://kong-gateway:8000/agents/agent-weather-mcp
```

Nasiko's orchestration engine **automatically** reads this variable upon boot, connects via the Gateway, lists the MCP tools available, and monkey-patches the LangChain `BaseChatModel` so your chosen AI has continuous access to the new capabilities.

No code changes are necessary in your agent to start using the tools!
