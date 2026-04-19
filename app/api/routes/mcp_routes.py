"""
MCP Routes - MCP server discovery and agent association endpoints.
"""

from fastapi import APIRouter, Depends, Path

from ..auth import get_user_id_from_token
from ..handlers import HandlerFactory
from ..types import (
    McpServerListResponse,
    McpManifestResponse,
    McpAssociationRequest,
    McpAssociationResponse,
)


def create_mcp_routes(handlers: HandlerFactory) -> APIRouter:
    """Create MCP-related routes."""
    router = APIRouter(prefix="/mcp", tags=["MCP"])

    @router.get(
        "/servers",
        response_model=McpServerListResponse,
        summary="List MCP Servers",
        description="List published MCP servers discoverable by the platform.",
    )
    async def list_mcp_servers(user_id: str = Depends(get_user_id_from_token)):
        _ = user_id
        return await handlers.mcp.list_mcp_servers()

    @router.get(
        "/{server_id}/manifest",
        response_model=McpManifestResponse,
        summary="Get MCP Manifest",
        description="Get the generated McpServerManifest.json for one MCP server.",
    )
    async def get_mcp_manifest(
        server_id: str = Path(..., description="MCP server id"),
        user_id: str = Depends(get_user_id_from_token),
    ):
        _ = user_id
        return await handlers.mcp.get_mcp_manifest(server_id)

    @router.put(
        "/agents/{agent_id}/associations",
        response_model=McpAssociationResponse,
        summary="Associate Agent With MCP Servers",
        description="Associate an existing agent with one or more MCP servers without code changes to the agent.",
    )
    async def associate_agent_with_mcp(
        association: McpAssociationRequest,
        agent_id: str = Path(..., description="Agent id"),
        user_id: str = Depends(get_user_id_from_token),
    ):
        _ = user_id
        return await handlers.mcp.associate_agent_with_mcp(agent_id, association)

    @router.get(
        "/agents/{agent_id}/associations",
        response_model=McpAssociationResponse,
        summary="Get Agent MCP Associations",
        description="Get MCP server associations configured for an agent.",
    )
    async def get_agent_mcp_associations(
        agent_id: str = Path(..., description="Agent id"),
        user_id: str = Depends(get_user_id_from_token),
    ):
        _ = user_id
        return await handlers.mcp.get_agent_mcp_associations(agent_id)

    return router
