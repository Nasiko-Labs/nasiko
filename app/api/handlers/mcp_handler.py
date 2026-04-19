"""
MCP Handler - MCP discovery, manifest, and agent association operations.
"""

from fastapi import HTTPException, status

from .base_handler import BaseHandler
from ..types import (
    McpServerListResponse,
    McpServerItemResponse,
    McpManifestResponse,
    McpAssociationRequest,
    McpAssociationResponse,
    McpAssociationItemResponse,
)


class McpHandler(BaseHandler):
    """Handler for MCP server and association operations."""

    async def list_mcp_servers(self) -> McpServerListResponse:
        try:
            servers = await self.service.list_mcp_servers()
            items = [McpServerItemResponse(**server) for server in servers]
            return McpServerListResponse(
                data=items,
                status_code=200,
                message="MCP servers retrieved successfully",
            )
        except Exception as e:
            await self.handle_service_error("list_mcp_servers", e)

    async def get_mcp_manifest(self, server_id: str) -> McpManifestResponse:
        try:
            manifest = await self.service.get_mcp_manifest(server_id)
            if not manifest:
                raise HTTPException(
                    status_code=status.HTTP_404_NOT_FOUND,
                    detail=f"MCP server '{server_id}' not found",
                )

            return McpManifestResponse(
                data=manifest,
                status_code=200,
                message="MCP manifest retrieved successfully",
            )
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("get_mcp_manifest", e)

    async def associate_agent_with_mcp(
        self, agent_id: str, association: McpAssociationRequest
    ) -> McpAssociationResponse:
        try:
            data = await self.service.associate_agent_with_mcp(
                agent_id=agent_id,
                mcp_server_ids=association.mcp_server_ids,
                replace=association.replace,
            )
            return McpAssociationResponse(
                data=McpAssociationItemResponse(**data),
                status_code=200,
                message="Agent MCP associations updated successfully",
            )
        except ValueError as e:
            raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e))
        except Exception as e:
            await self.handle_service_error("associate_agent_with_mcp", e)

    async def get_agent_mcp_associations(self, agent_id: str) -> McpAssociationResponse:
        try:
            data = await self.service.get_agent_mcp_associations(agent_id)
            return McpAssociationResponse(
                data=McpAssociationItemResponse(**data),
                status_code=200,
                message="Agent MCP associations retrieved successfully",
            )
        except ValueError as e:
            raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail=str(e))
        except Exception as e:
            await self.handle_service_error("get_agent_mcp_associations", e)
