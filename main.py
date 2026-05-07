# How to Publish an MCP Server on Nasiko

This guide explains how to publish an MCP (Model Context Protocol) server as a
deployable artifact on the Nasiko platform, and how existing agents can consume
its tools.

## Prerequisites

- The official Python MCP SDK (`pip install "mcp[cli]>=1.0"`)
- A basic understanding of MCP's tool/resource/prompt decorators

## Step 1: Create Your MCP Server

Follow the standard Nasiko project structure:

```
my-mcp-server/
├── docker-compose.yml
├── Dockerfile
└── src/
    └── main.py          ← your MCP server code
```

### Example `src/main.py`

```python
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("my-tools")

@mcp.tool()
def add(a: int, b: int) -> int:
    """Add two numbers together."""
    return a + b

@mcp.resource("config://settings")
def get_settings() -> str:
    """Return server configuration."""
    return '{"precision": 10}'

@mcp.prompt()
def math_helper(problem: str) -> str:
    """Generate a math problem-solving prompt."""
    return f"Solve: {problem}"

if __name__ == "__main__":
    mcp.run()
```

### Example `Dockerfile`

```dockerfile
FROM python:3.11-slim
WORKDIR /app
COPY src/ src/
RUN pip install --no-cache-dir "mcp[cli]>=1.0"
CMD ["python", "src/main.py"]
```

### Example `docker-compose.yml`

```yaml
version: '3.8'
services:
  my-tools:
    build: .
    stdin_open: true
```

## Step 2: Upload to Nasiko

### Option A: Via API

```bash
# Zip your project
zip -r my-mcp-server.zip my-mcp-server/

# Upload via the ingest endpoint
curl -X POST http://localhost:8000/ingest \
  -F "file=@my-mcp-server.zip;type=application/zip"
```

### Option B: Via the Nasiko Web App

Navigate to the upload page and drag-and-drop your zip file.

## What Happens Automatically

When you upload an MCP server, Nasiko's pipeline does the following — **no
manual steps required**:

1. **R1 — Artifact Detection**: AST-based static analysis recognizes `mcp` or
   `fastmcp` imports and classifies the upload as `MCP_SERVER`. No flag needed.

2. **R3 — Manifest Generation**: A `McpServerManifest.json` is auto-generated
   capturing all `@mcp.tool()`, `@mcp.resource()`, and `@mcp.prompt()`
   definitions, including parameter types and docstrings.

3. **R2 — Bridge Deployment**: A stdio-to-HTTP bridge spawns your server as a
   subprocess, performs the MCP JSON-RPC 2.0 handshake, and exposes it via
   FastAPI HTTP endpoints.

4. **Kong Registration**: The server is registered with Kong so it becomes
   discoverable and routable at `/mcp/{artifact_id}/`.

5. **R5 — Observability**: OpenTelemetry tracing is injected into the bridge
   so every tool call produces spans in Arize Phoenix — with no code changes
   to your server.

## Step 3: How an Agent Consumes MCP Tools

### Linking an Agent to an MCP Server

After your MCP server is deployed, link an existing agent to it:

```bash
curl -X POST http://localhost:8000/agent/link \
  -H "Content-Type: application/json" \
  -d '{
    "agent_artifact_id": "my-langchain-agent",
    "mcp_artifact_id": "<your-mcp-artifact-id>"
  }'
```

The response includes the list of available tools from the MCP server.

### Zero-Code Tool Injection

Nasiko injects MCP tools into agents at runtime — **no code changes to your
agent are required**. Both LangChain and CrewAI agents receive proxy tool
wrappers that forward calls through the bridge:

- **LangChain**: Tools are injected as `StructuredTool` instances
- **CrewAI**: Tools are injected as `BaseTool` subclasses

### Calling a Tool Directly

You can also call MCP tools directly via the bridge API:

```bash
curl -X POST http://localhost:8000/mcp/<artifact_id>/call \
  -H "Content-Type: application/json" \
  -H "traceparent: 00-<trace-id>-<span-id>-01" \
  -d '{
    "tool_name": "add",
    "arguments": {"a": 40, "b": 2}
  }'
```

The `traceparent` header enables end-to-end trace correlation in Phoenix.

## Transport Details

- **stdio** is the primary transport. Servers that speak MCP over stdio are
  fully supported out of the box.
- The platform **automatically detects** the transport from the source code.
- A stdio-to-HTTP bridge converts subprocess I/O into HTTP endpoints, and this
  bridge is where observability instrumentation lives.

## Viewing Your Server

Once published, your MCP server appears in:

- **Web App**: Listed alongside agents as a first-class artifact
- **CLI**: `nasiko mcp list` shows all published MCP servers
- **Manifest**: `nasiko mcp manifest <id>` shows the auto-generated manifest
- **Traces**: All tool calls appear in the Phoenix UI under project `mcp-bridge`

## Troubleshooting

| Issue | Solution |
|-------|----------|
| Upload returns 422 with AMBIGUOUS_ARTIFACT | Your code imports both MCP and another framework (LangChain/CrewAI). An MCP server must only import `mcp` or `fastmcp`. |
| Upload returns 422 with MISSING_STRUCTURE | Ensure your project has `src/main.py`, `Dockerfile`, and `docker-compose.yml`. |
| Manifest has no tools | Check that your functions use `@mcp.tool()` decorators (not just plain functions). |
| Tool calls fail with "Bridge not running" | The MCP server subprocess may have crashed. Check its stderr logs. |
