"""
N8N Routes - All n8n related API endpoints including credentials and workflow registration
"""

from fastapi import APIRouter, Path, Query, Depends
from typing import Optional
from app.api.types import SuccessResponse
from app.api.auth import get_user_id_from_token
from app.entity.n8n_entity import (
    N8nRegisterRequest, N8nRegisterResponse,
    UserN8NCredentialCreateRequest, UserN8NCredentialUpdateRequest,
    UserN8NCredentialTestRequest, UserN8NCredentialResponse,
    UserN8NCredentialSingleResponse, UserN8NConnectResponse, WorkflowListResponse
)

def create_n8n_routes(handlers) -> APIRouter:
    """Create n8n-specific routes"""
    router = APIRouter(prefix="/agents/n8n", tags=["N8N Operations"])
    
    # Use n8n handler from factory
    n8n_handler = handlers.n8n
    
    # Agent Registration
    @router.post("/register", response_model=N8nRegisterResponse)
    async def register_n8n_workflow(
        request: N8nRegisterRequest,
        user_id: str = Depends(get_user_id_from_token)
    ):
        """Register n8n workflow as a2a agent with user ownership"""
        return await n8n_handler.register_workflow_as_agent(request, user_id)

    # User Credentials Management
    @router.post(
        "/connect",
        response_model=UserN8NConnectResponse,
        status_code=201,
        summary="Save User N8N Credentials",
        description="Test N8N connection first, then save credentials only if test succeeds. Uses authenticated user."
    )
    async def save_user_n8n_credentials(
        credential_data: UserN8NCredentialCreateRequest,
        user_id: str = Depends(get_user_id_from_token)
    ):
        """
        Test N8N connection first, then save credentials only if successful.
        
        - **connection_name**: User-defined name for this connection
        - **n8n_url**: N8N instance URL (protocol will be added if missing)
        - **api_key**: N8N API key (will be encrypted before storage)
        
        The endpoint will:
        1. Validate the N8N URL format
        2. **Test the connection first** to verify credentials work
        3. **Only save credentials if test succeeds** (encrypted storage)
        4. Return success/failure with connection details
        
        **Important**: Credentials are only saved if the connection test passes!
        User ID is automatically extracted from JWT token.
        """
        return await n8n_handler.create_or_update_credential(credential_data, user_id)

    @router.get(
        "/credentials",
        response_model=UserN8NCredentialSingleResponse,
        summary="Get User N8N Credentials",
        description="Get authenticated user's saved N8N credentials (without sensitive data)"
    )
    async def get_user_n8n_credentials(
        user_id: str = Depends(get_user_id_from_token)
    ):
        """
        Retrieve user's N8N credentials information.
        
        Returns credential information including:
        - N8N URL
        - Connection status
        - Last tested time
        - Creation/update timestamps
        
        Note: API key is not returned for security reasons.
        User ID is automatically extracted from JWT token.
        """
        return await n8n_handler.get_user_credential(user_id)

    @router.put(
        "/credentials",
        response_model=UserN8NCredentialSingleResponse,
        summary="Update User N8N Credentials",
        description="Update authenticated user's N8N credentials"
    )
    async def update_user_n8n_credentials(
        update_data: UserN8NCredentialUpdateRequest,
        user_id: str = Depends(get_user_id_from_token)
    ):
        """
        Update user's N8N credentials.
        
        Can update:
        - **n8n_url**: New N8N instance URL
        - **api_key**: New API key
        - **is_active**: Enable/disable credential
        
        If URL or API key is updated, connection will be tested automatically.
        User ID is automatically extracted from JWT token.
        """
        return await n8n_handler.update_credential(user_id, update_data)

    @router.delete(
        "/credentials",
        response_model=SuccessResponse,
        summary="Delete User N8N Credentials",
        description="Delete authenticated user's N8N credentials permanently"
    )
    async def delete_user_n8n_credentials(
        user_id: str = Depends(get_user_id_from_token)
    ):
        """
        Delete user's N8N credentials permanently.
        
        This action cannot be undone. The user will need to re-enter
        their credentials to use N8N integration features.
        User ID is automatically extracted from JWT token.
        """
        return await n8n_handler.delete_credential(user_id)

    @router.get(
        "/workflows",
        response_model=WorkflowListResponse,
        summary="List N8N Workflows",
        description="List workflows for authenticated user's N8N connection"
    )
    async def list_workflows(
        active_only: bool = Query(True, description="Filter to show only active workflows"),
        limit: int = Query(100, description="Maximum number of workflows to return"),
        user_id: str = Depends(get_user_id_from_token)
    ):
        """
        List workflows for authenticated user's N8N connection.
        
        Query Parameters:
        - **active_only**: Filter to show only active workflows (default: true)
        - **limit**: Maximum number of workflows to return (default: 100)
        
        Returns a simplified list of workflows with:
        - Workflow ID and name
        - Active status
        - Whether it's a chat workflow
        - Number of nodes
        - Last updated timestamp
        - Tags
        
        User ID is automatically extracted from JWT token.
        """
        return await n8n_handler.list_workflows(active_only, limit, user_id)
    
    return router