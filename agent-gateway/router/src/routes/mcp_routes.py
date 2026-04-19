"""
MCP Router Endpoints
FastAPI endpoints for MCP server communication.
Can be included in main.py of agent-gateway/router/src/main.py
"""

from fastapi import APIRouter, HTTPException
from typing import Dict, Any, Optional

from router.src.services.mcp_router_service import MCPRouterService

# Router for MCP endpoints
mcp_router = APIRouter(prefix="/mcp", tags=["MCP"])

_mcp_service: Optional[MCPRouterService] = None


def set_mcp_service(service: MCPRouterService) -> None:
    """Bind MCP service instance at application startup."""
    global _mcp_service
    _mcp_service = service


def _get_mcp_service() -> MCPRouterService:
    if _mcp_service is None:
        raise HTTPException(status_code=503, detail="MCP service is not initialized")
    return _mcp_service


@mcp_router.get("/servers")
async def list_mcp_servers() -> Dict[str, Any]:
    """
    List all available MCP servers.
    
    Returns:
        Dict with server metadata including tool counts
    """
    mcp_service = _get_mcp_service()
    servers = await mcp_service.list_mcp_servers()
    return {"status": "success", "servers": servers}


@mcp_router.get("/{server_id}/manifest")
async def get_mcp_manifest(server_id: str) -> Dict[str, Any]:
    """
    Get the manifest for an MCP server.
    
    Args:
        server_id: The MCP server ID (agent_name)
        
    Returns:
        MCP Server Manifest with tools, resources, prompts
    """
    mcp_service = _get_mcp_service()
    manifest = await mcp_service.get_mcp_manifest(server_id)
    if not manifest:
        raise HTTPException(status_code=404, detail="MCP server not found")
    return manifest


@mcp_router.post("/{server_id}/tool")
async def call_mcp_tool(
    server_id: str,
    request: Dict[str, Any],
) -> Dict[str, Any]:
    """
    Call an MCP tool on a specific server.
    
    Args:
        server_id: The MCP server ID (agent_name)
        request: Request body with:
            - tool_name: Name of the tool to call
            - arguments: Tool arguments (dict)
    
    Returns:
        Tool result or error
    """
    mcp_service = _get_mcp_service()
    tool_name = request.get("tool_name")
    arguments = request.get("arguments", {})

    if not tool_name:
        raise HTTPException(status_code=400, detail="tool_name is required")

    response = await mcp_service.call_mcp_tool(server_id, tool_name, arguments)
    response_payload = response.model_dump()
    if not response_payload.get("success"):
        raise HTTPException(status_code=502, detail=response_payload.get("error"))
    return response_payload
