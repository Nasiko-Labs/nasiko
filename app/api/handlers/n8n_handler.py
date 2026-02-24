"""
N8N Handler - Handles all n8n related operations including workflow registration
"""

from fastapi import HTTPException, status
from .base_handler import BaseHandler
from ..types import SuccessResponse
from app.entity.n8n_entity import (
    N8nRegisterRequest, N8nRegisterResponse,
    UserN8NCredentialCreateRequest, UserN8NCredentialUpdateRequest, 
    UserN8NCredentialTestRequest, UserN8NCredentialResponse,
    UserN8NCredentialSingleResponse, WorkflowListResponse
)
from typing import Optional, Dict, Any

class N8nHandler(BaseHandler):
    """Handler for all n8n operations"""

    def __init__(self, service, logger):
        super().__init__(service, logger)

    async def register_workflow_as_agent(self, request: N8nRegisterRequest, user_id: str) -> N8nRegisterResponse:
        """Register n8n workflow as a2a agent with user ownership"""
        try:
            self.log_info("Registering n8n workflow as a2a agent", 
                         user_id=user_id, 
                         workflow_id=request.workflow_id)
            
            # Get user's N8N credentials from repository
            user_credential = await self.service.repo.get_user_n8n_credential_decrypted(user_id)
            if not user_credential:
                return N8nRegisterResponse(
                    success=False,
                    message=f"No N8N credentials found for user {user_id}. Please connect to N8N first.",
                )
            
            # Create n8n service with user's credentials
            from app.service.n8n_service import N8nService
            n8n_service = N8nService(
                base_url=user_credential["n8n_url"],
                api_key=user_credential["api_key"],
                logger=self.logger
            )
            
            # Call the service method to handle the registration logic
            result = await n8n_service.register_workflow_as_agent(request, user_id, self.service.repo)
            
            # Convert service result to handler response format
            if result["success"]:
                return N8nRegisterResponse(
                    success=True,
                    message=result["message"],
                    agent_name=result["agent_name"],
                    agent_id=result["agent_id"],
                    webhook_url=result["webhook_url"],
                    container_name=result["container_name"],
                    upload_id=result["upload_id"]
                )
            else:
                return N8nRegisterResponse(
                    success=False,
                    message=result["message"]
                )
            
        except Exception as e:
            await self.handle_service_error("register_workflow_as_agent", e)

    # User N8N Credentials Management
    async def create_or_update_credential(self, request: UserN8NCredentialCreateRequest, user_id: str) -> Dict[str, Any]:
        """Create or update user N8N credential with connection testing"""
        try:
            self.log_info("Creating/updating N8N credential", user_id=user_id)
            
            # Test connection first before saving
            from app.service.n8n_service import N8nService
            n8n_service = N8nService(
                base_url=request.n8n_url,
                api_key=request.api_key,
                logger=self.logger
            )
            
            # Test the connection
            test_result = await n8n_service.test_connection()
            if not test_result.get("success", False):
                raise HTTPException(
                    status_code=status.HTTP_400_BAD_REQUEST,
                    detail=f"N8N connection test failed: {test_result.get('message', 'Unknown error')}"
                )
            
            # Prepare credential data (repository handles encryption)
            from datetime import datetime, timezone
            credential_data = {
                "user_id": user_id,
                "connection_name": request.connection_name,
                "n8n_url": request.n8n_url,
                "api_key": request.api_key,  # Repository will encrypt this
                "credential_type": "n8n",
                "is_active": True,
                "last_tested": datetime.now(timezone.utc),
                "created_at": datetime.now(timezone.utc),
                "updated_at": datetime.now(timezone.utc)
            }
            
            # Save or update credential
            result = await self.service.repo.upsert_user_n8n_credential(credential_data)
            if not result:
                raise HTTPException(
                    status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                    detail="Failed to save N8N credentials"
                )
            
            # Return simplified response format for /connect endpoint
            return {
                "data": {
                    "connection_name": result["connection_name"],
                    "connection_status": "success"
                },
                "status_code": 201,
                "message": "N8N connection successful and credentials saved securely"
            }
            
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("create_or_update_credential", e)
    
    async def test_connection(self, request: UserN8NCredentialTestRequest) -> UserN8NCredentialResponse:
        """Test N8N connection for user's saved credentials"""
        try:
            self.log_info("Testing N8N connection", user_id=request.user_id)
            
            # Get user's stored credentials
            user_credential = await self.service.repo.get_user_n8n_credential_decrypted(request.user_id)
            if not user_credential:
                return UserN8NCredentialResponse(
                    success=False,
                    message=f"No N8N credentials found for user {request.user_id}"
                )
            
            # Test connection using stored credentials
            from app.service.n8n_service import N8nService
            n8n_service = N8nService(
                base_url=user_credential["n8n_url"],
                api_key=user_credential["api_key"],
                logger=self.logger
            )
            
            test_result = await n8n_service.test_connection()
            return UserN8NCredentialResponse(
                success=test_result.get("success", False),
                message=test_result.get("message", "Connection test completed")
            )
            
        except Exception as e:
            await self.handle_service_error("test_connection", e)
    
    async def get_user_credential(self, user_id: str) -> UserN8NCredentialSingleResponse:
        """Get user's N8N credential (without sensitive data)"""
        try:
            self.log_debug("Retrieving N8N credential", user_id=user_id)
            
            # Get credential from repository
            credential = await self.service.repo.get_user_n8n_credential_by_user_id(user_id)
            if not credential:
                raise HTTPException(
                    status_code=status.HTTP_404_NOT_FOUND,
                    detail=f"No N8N credentials found for user {user_id}"
                )
            
            return UserN8NCredentialSingleResponse(
                success=True,
                message="N8N credential retrieved successfully",
                data=UserN8NCredentialResponse(
                    success=True,
                    message="Credential data retrieved",
                    user_id=credential["user_id"],
                    connection_name=credential["connection_name"],
                    n8n_url=credential["n8n_url"],
                    is_active=credential.get("is_active", True),
                    last_tested=credential.get("last_tested"),
                    created_at=credential.get("created_at"),
                    updated_at=credential.get("updated_at")
                )
            )
            
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("get_user_credential", e)
    
    async def update_credential(self, user_id: str, request: UserN8NCredentialUpdateRequest) -> UserN8NCredentialSingleResponse:
        """Update user's N8N credential"""
        try:
            self.log_info("Updating N8N credential", user_id=user_id)
            
            # Check if credential exists
            existing = await self.service.repo.get_user_n8n_credential_by_user_id(user_id)
            if not existing:
                raise HTTPException(
                    status_code=status.HTTP_404_NOT_FOUND,
                    detail=f"No N8N credentials found for user {user_id}"
                )
            
            # Test new connection if URL or API key provided
            if request.n8n_url or request.api_key:
                from app.service.n8n_service import N8nService
                test_url = request.n8n_url or existing["n8n_url"]
                
                # Get existing api key if not provided in update
                if request.api_key:
                    test_api_key = request.api_key
                else:
                    # Get decrypted credential to extract api key
                    decrypted_cred = await self.service.repo.get_user_n8n_credential_decrypted(user_id)
                    test_api_key = decrypted_cred.get("api_key") if decrypted_cred else None
                
                n8n_service = N8nService(
                    base_url=test_url,
                    api_key=test_api_key,
                    logger=self.logger
                )
                
                test_result = await n8n_service.test_connection()
                if not test_result.get("success", False):
                    raise HTTPException(
                        status_code=status.HTTP_400_BAD_REQUEST,
                        detail=f"N8N connection test failed: {test_result.get('message', 'Unknown error')}"
                    )
            
            # Update credential data
            from datetime import datetime, timezone
            update_data = {"updated_at": datetime.now(timezone.utc)}
            
            if request.n8n_url:
                update_data["n8n_url"] = request.n8n_url
            if request.api_key:
                update_data["api_key"] = request.api_key  # Repository will handle encryption
                update_data["last_tested"] = datetime.now(timezone.utc)
            if request.is_active is not None:
                update_data["is_active"] = request.is_active
            
            result = await self.service.repo.update_user_n8n_credential(user_id, update_data)
            if not result:
                raise HTTPException(
                    status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                    detail="Failed to update N8N credentials"
                )
            
            return UserN8NCredentialSingleResponse(
                success=True,
                message="N8N credentials updated successfully",
                data=UserN8NCredentialResponse(
                    success=True,
                    message="Credential data updated",
                    user_id=result["user_id"],
                    connection_name=result["connection_name"],
                    n8n_url=result["n8n_url"],
                    is_active=result.get("is_active", True),
                    last_tested=result.get("last_tested"),
                    created_at=result.get("created_at"),
                    updated_at=result.get("updated_at")
                )
            )
            
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("update_credential", e)
    
    async def delete_credential(self, user_id: str) -> SuccessResponse:
        """Delete user's N8N credential"""
        try:
            self.log_info("Deleting N8N credential", user_id=user_id)
            
            # Check if credential exists
            existing = await self.service.repo.get_user_n8n_credential_by_user_id(user_id)
            if not existing:
                raise HTTPException(
                    status_code=status.HTTP_404_NOT_FOUND,
                    detail=f"No N8N credentials found for user {user_id}"
                )
            
            # Delete credential
            result = await self.service.repo.delete_user_n8n_credential(user_id)
            if not result:
                raise HTTPException(
                    status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                    detail="Failed to delete N8N credentials"
                )
            
            return SuccessResponse(
                success=True,
                message="N8N credentials deleted successfully"
            )
            
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("delete_credential", e)
    
    async def list_workflows(self, active_only: bool, limit: int, user_id: str) -> WorkflowListResponse:
        """List workflows for user's N8N connection"""
        try:
            self.log_info("Listing workflows for N8N connection", user_id=user_id)
            
            # Get user's credentials
            user_credential = await self.service.repo.get_user_n8n_credential_decrypted(user_id)
            if not user_credential:
                raise ValueError(f"No N8N credentials found for user {user_id}")
            
            # List workflows using N8N service
            from app.service.n8n_service import N8nService
            n8n_service = N8nService(
                base_url=user_credential["n8n_url"],
                api_key=user_credential["api_key"],
                logger=self.logger
            )
            
            workflows = await n8n_service.get_workflows()
            
            # Filter and format workflows according to your specification
            workflow_list = []
            for workflow in workflows:
                if active_only and not workflow.get("active", True):
                    continue
                    
                workflow_item = {
                    "id": workflow.get("id"),
                    "name": workflow.get("name"),
                    "active": workflow.get("active", True),
                    "is_chat_workflow": workflow.get("is_chat_workflow", False),
                    "nodes_count": workflow.get("nodes_count", 0),
                    "last_updated": workflow.get("updatedAt"),
                    "tags": workflow.get("tags", [])
                }
                workflow_list.append(workflow_item)
            
            # Limit results
            if limit:
                workflow_list = workflow_list[:limit]
            
            return WorkflowListResponse(
                workflows=workflow_list,
                total_count=len(workflow_list),
                connection_name=user_credential["connection_name"],
                message=f"Found {len(workflow_list)} workflows"
            )
            
        except ValueError as e:
            # Handle validation errors (like credential not found, connection name mismatch)
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=str(e)
            )
        except Exception as e:
            await self.handle_service_error("list_workflows", e)