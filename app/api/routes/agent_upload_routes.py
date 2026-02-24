"""
Agent Upload Routes - Agent upload and status tracking endpoints
"""

from fastapi import APIRouter, File, Form, UploadFile, Path, Query, Response, Depends
from ..handlers import HandlerFactory
from ..types import (
    AgentUploadResponse, AgentDirectoryUploadRequest,
    UploadStatusSingleResponse, UploadStatusUpdateRequest, SimpleUserUploadAgentsResponse
)
from ..auth import get_user_id_from_token
from typing import Optional

def create_agent_upload_routes(handlers: HandlerFactory) -> APIRouter:
    """Create agent upload routes"""
    router = APIRouter(tags=["Agent Upload"])

    @router.post(
        "/agents/upload",
        response_model=AgentUploadResponse,
        summary="Upload Agent Zip",
        description="Upload agent from zip file"
    )
    async def upload_agent_zip(
        response: Response,
        file: UploadFile = File(..., description="Agent .zip file"),
        agent_name: Optional[str] = Form(None, description="Optional agent name"),
        user_id: str = Depends(get_user_id_from_token)
    ):
        result = await handlers.agent_upload.upload_agent_zip(file, user_id, agent_name)
        response.status_code = result.status_code
        return result

    @router.post(
        "/agents/upload-directory", 
        response_model=AgentUploadResponse,
        summary="Upload Agent Directory",
        description="Upload agent from local directory path"
    )
    async def upload_agent_directory(
        upload_request: AgentDirectoryUploadRequest, 
        response: Response,
        user_id: str = Depends(get_user_id_from_token)
    ):
        result = await handlers.agent_upload.upload_agent_directory(
            upload_request.directory_path, user_id, upload_request.agent_name
        )
        response.status_code = result.status_code
        return result

    @router.put(
        "/upload-status/agent/{agent_name}/latest",
        response_model=UploadStatusSingleResponse,
        summary="Update Latest Upload Status",
        description="Update the latest upload status for an agent (used by orchestrator)"
    )
    async def update_upload_status_by_agent_latest(
        update_data: UploadStatusUpdateRequest,
        agent_name: str = Path(..., description="Agent name"),
    ):
        return await handlers.agent_upload.update_upload_status_by_agent_latest(agent_name, update_data)

    # User Upload Agents Endpoint
    @router.get(
        "/user/upload-agents",
        response_model=SimpleUserUploadAgentsResponse,
        summary="Get My Upload Agents",
        description="Get all agents uploaded by the authenticated user with simplified format"
    )
    async def get_user_upload_agents(
        limit: int = Query(100, description="Maximum number of agents to return"),
        user_id: str = Depends(get_user_id_from_token)
    ):
        print("user_id", user_id)
        return await handlers.agent_upload.get_user_upload_agents(user_id, limit)

    # Agent Files Download Endpoint (for BuildKit)
    @router.get(
        "/agents/{agent_name}/download",
        summary="Download Agent Files",
        description="Download agent files as a tarball for BuildKit builds"
    )
    async def download_agent_files(
        agent_name: str = Path(..., description="Agent name"),
        version: Optional[str] = Query(None, description="Agent version (e.g., '1.0.0')")
    ):
        return await handlers.agent_upload.download_agent_files(agent_name, version)

    return router