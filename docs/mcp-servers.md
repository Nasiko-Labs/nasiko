# MCP Server Developer Guide

This guide explains how Nasiko handles MCP servers alongside standard agents.

## What changes and what does not

- **Upload API stays the same.** Use the same CLI commands and backend upload endpoints you already use for agents.
- **Artifact detection is automatic.** Nasiko detects MCP servers from their source markers or from an existing `MCPManifest.json`.
- **Routing is different.** Standard agents keep the `/agents/<id>` path. MCP servers are published under `/mcp/<id>`.

## Supported MCP server artifact structure

Minimum recommended layout:

```text
my-mcp-server/
├── Dockerfile
├── pyproject.toml            # or requirements.txt
├── MCPManifest.json          # optional; auto-generated if missing
├── src/
│   ├── main.py               # or __main__.py / bridge.py / mcp_bridge.py
│   └── ...
└── README.md
```

Nasiko treats an upload as an MCP server when it finds MCP markers such as:

- `from fastmcp import FastMCP`
- `from mcp import ...`
- `@mcp.tool()` or `@FastMCP.tool()`
- an existing `MCPManifest.json`

If the manifest is missing, Nasiko will generate one during upload.

## How to publish an MCP server

1. Package your MCP server in a directory or ZIP archive.
2. Make sure the entrypoint exposes your tools through FastMCP or another MCP-compatible implementation.
3. Upload it with the same command you already use for agents:

```bash
nasiko agent upload-directory ./my-mcp-server --name my-mcp-server
nasiko agent upload-zip my-mcp-server.zip --name my-mcp-server
```

4. Nasiko will:
   - detect the artifact as `mcp_server`
   - generate `MCPManifest.json` if needed
   - store artifact metadata in the registry
   - publish the server through Kong at `/mcp/<name>`

## How to consume MCP tools from an existing agent

An existing agent can consume MCP tools by calling the published MCP endpoint through Kong or by using the internal service URL inside the cluster.

Recommended flow:

1. Look up the MCP server in the registry.
2. Read the `mcp_manifest` and `artifact_type` fields.
3. Use an MCP-compatible client or adapter to list tools and invoke them.
4. If you want a stable relationship in the registry, associate the agent with the MCP server using the CLI.

Example pattern:

```python
# Pseudocode: use your MCP SDK or adapter of choice
mcp_url = "http://localhost:9100/mcp/weather-tools"

client = create_mcp_client(mcp_url)
tools = client.list_tools()
result = client.call_tool("get_weather", {"city": "Seattle"})
```

If both artifacts are managed in Nasiko, you can also store an association in registry metadata so the agent can discover its MCP dependencies later.

## Supported MCP server flow in Nasiko

1. Upload the artifact with the existing upload API.
2. Nasiko validates the folder or ZIP and determines whether it is an agent or MCP server.
3. MCP uploads get an MCP manifest generated or loaded.
4. The orchestrator deploys the container with MCP-specific runtime hints.
5. The registry stores `artifact_type = mcp_server`, manifest metadata, and associations.
6. Kong publishes the route as `/mcp/<artifact-name>`.

## Notes for developers

- Existing agent uploads are unchanged.
- Existing agent routes are unchanged.
- Only MCP artifacts opt into the new `/mcp/...` Kong route.
- If you are testing locally, verify the route with:

```bash
curl http://localhost:9100/mcp/my-mcp-server/health
```
