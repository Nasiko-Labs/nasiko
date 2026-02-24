"""
Agent Update Routes - Agent update, rollback, and version management endpoints
"""

from fastapi import APIRouter, File, Form, UploadFile, Path, Depends, HTTPException
from ..handlers import HandlerFactory
from ..types import (
    AgentUpdateRequest, AgentUpdateResponse, AgentRollbackRequest, AgentRollbackResponse,
    AgentVersionHistoryResponse
)
from ..auth import get_user_id_from_token
from typing import Optional


def create_agent_update_routes(handlers: HandlerFactory) -> APIRouter:
    """Create agent update routes"""
    router = APIRouter(prefix="/agents", tags=["Agent Updates"])

    @router.put(
        "/{agent_id}/update",
        response_model=AgentUpdateResponse,
        summary="Update Agent",
        description="Update an existing agent with new code"
    )
    async def update_agent(
        agent_id: str = Path(..., description="Agent ID to update"),
        file: Optional[UploadFile] = File(None, description="New agent code .zip file (optional for GitHub-sourced agents)"),
        version: Optional[str] = Form("auto", description="Version strategy: auto, major, minor, patch, or specific version"),
        update_strategy: str = Form("rolling", description="Deployment strategy: rolling or blue-green"),
        cleanup_old: bool = Form(True, description="Whether to cleanup old deployments"),
        description: Optional[str] = Form(None, description="Update description"),
        user_id: str = Depends(get_user_id_from_token)
    ):
        """Update an existing agent with new code"""
        update_request = AgentUpdateRequest(
            version=version,
            update_strategy=update_strategy,
            cleanup_old=cleanup_old,
            description=description
        )
        
        return await handlers.agent_update.update_agent(agent_id, file, user_id, update_request)

    @router.post(
        "/{agent_id}/rollback",
        response_model=AgentRollbackResponse,
        summary="Rollback Agent",
        description="Rollback an agent to a previous version"
    )
    async def rollback_agent(
        rollback_request: AgentRollbackRequest,
        agent_id: str = Path(..., description="Agent ID to rollback"),
        user_id: str = Depends(get_user_id_from_token)
    ):
        """Rollback an agent to a previous version"""
        return await handlers.agent_update.rollback_agent(agent_id, user_id, rollback_request)

    @router.get(
        "/{agent_id}/versions",
        response_model=AgentVersionHistoryResponse,
        summary="Get Version History",
        description="Get version history for an agent"
    )
    async def get_version_history(
        agent_id: str = Path(..., description="Agent ID"),
        user_id: str = Depends(get_user_id_from_token)
    ):
        """Get version history for an agent"""
        return await handlers.agent_update.get_version_history(agent_id)

    return router