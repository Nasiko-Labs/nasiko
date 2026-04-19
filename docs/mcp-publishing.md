# Publish MCP Servers In Nasiko

This guide shows how to publish an MCP server and associate it with an existing agent without code changes to the agent.

## 1. MCP Project Layout

Uploaded MCP artifacts must follow the same project contract as agents:

- docker-compose.yml
- Dockerfile
- src/main.py (or equivalent supported main entrypoint)

## 2. Publish MCP Server

You can upload from zip or directory using existing upload paths.

Example (directory upload through API-compatible CLI flow):

```bash
nasiko agent upload-directory ./examples/mcp-weather-server --name weather-mcp
```

On successful upload, Nasiko will:

- detect artifact type as MCP automatically
- validate structure
- auto-generate McpServerManifest.json when needed
- trigger orchestration for MCP publish stages
- expose bridge endpoint metadata in API responses

## 3. Discover Published MCP Servers

```bash
nasiko mcp list
```

To inspect one manifest:

```bash
nasiko mcp manifest weather-mcp
```

## 4. Associate Agent With MCP Servers

Associate one agent with one or more published MCP servers:

```bash
nasiko mcp associate my-agent-id weather-mcp
```

Replace existing associations entirely:

```bash
nasiko mcp associate my-agent-id weather-mcp --replace
```

View current associations:

```bash
nasiko mcp associations my-agent-id
```

## 5. Agent Consumption Path

Agent-to-MCP linkage is stored as registry metadata (associated_mcp_servers and mcp_bridge_urls).

When the router selects an agent, it now reads those association fields from the registry response and forwards them in JSON-RPC request metadata under metadata.mcp.

At runtime, MCP tools are reachable via bridge endpoints exposed through the router path:

- /router/mcp/{server_id}/tool

This keeps agent code unchanged while enabling centralized MCP capability discovery and routing.
