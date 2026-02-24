"""
Agent Update Handler - Manages agent update and rollback operations
"""

from fastapi import HTTPException, status, UploadFile
from .base_handler import BaseHandler
from ..types import (
    AgentUpdateRequest, AgentUpdateResponse, AgentRollbackRequest, AgentRollbackResponse,
    AgentVersionHistoryResponse, AgentVersionInfo
)
from typing import Optional


class AgentUpdateHandler(BaseHandler):
    """Handler for agent update and rollback operations"""

    def __init__(self, service, logger):
        super().__init__(service, logger)
        from app.service.agent_update_service import AgentUpdateService
        self.update_service = AgentUpdateService(logger, service.repo)

    async def update_agent(
        self, 
        agent_id: str, 
        file: Optional[UploadFile], 
        user_id: str,
        update_request: AgentUpdateRequest
    ) -> AgentUpdateResponse:
        """Update an existing agent with new code"""
        try:
            self.log_info("Updating agent", agent_id=agent_id, user_id=user_id, 
                         version_strategy=update_request.version, 
                         update_strategy=update_request.update_strategy)
            
            # Validate file if provided
            if file:
                if not file.filename:
                    raise HTTPException(
                        status_code=status.HTTP_400_BAD_REQUEST,
                        detail="No file provided"
                    )
                
                if not file.filename.endswith('.zip'):
                    raise HTTPException(
                        status_code=status.HTTP_400_BAD_REQUEST,
                        detail="Only .zip files are supported"
                    )
            
            # Process the update
            result = await self.update_service.update_agent(
                agent_id=agent_id,
                file=file,
                user_id=user_id,
                version=update_request.version,
                update_strategy=update_request.update_strategy,
                cleanup_old=update_request.cleanup_old,
                description=update_request.description
            )
            
            if result.success:
                return AgentUpdateResponse(
                    message=f"Agent {agent_id} update initiated successfully",
                    agent_id=result.agent_id,
                    new_version=result.new_version,
                    previous_version=result.previous_version,
                    build_id=result.build_id,
                    deployment_id=result.deployment_id,
                    update_strategy=result.update_strategy,
                    status=result.status,
                    status_code=202  # Accepted - processing
                )
            else:
                return AgentUpdateResponse(
                    message=f"Agent {agent_id} update failed: {result.error_message}",
                    agent_id=result.agent_id,
                    new_version=result.new_version,
                    previous_version=result.previous_version,
                    update_strategy=result.update_strategy,
                    status=result.status,
                    status_code=400
                )
                
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Agent update failed", agent_id=agent_id, error=str(e))
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Agent update failed: {str(e)}"
            )

    async def rollback_agent(
        self, 
        agent_id: str, 
        user_id: str,
        rollback_request: AgentRollbackRequest
    ) -> AgentRollbackResponse:
        """Rollback an agent to a previous version"""
        try:
            self.log_info("Rolling back agent", agent_id=agent_id, user_id=user_id,
                         target_version=rollback_request.target_version,
                         reason=rollback_request.reason)
            
            result = await self.update_service.rollback_agent(
                agent_id=agent_id,
                user_id=user_id,
                target_version=rollback_request.target_version,
                cleanup_failed=rollback_request.cleanup_failed,
                reason=rollback_request.reason
            )
            
            if result.success:
                return AgentRollbackResponse(
                    message=f"Agent {agent_id} rollback initiated successfully",
                    agent_id=result.agent_id,
                    rolled_back_to=result.new_version,
                    rolled_back_from=result.previous_version,
                    status=result.status,
                    status_code=202  # Accepted - processing
                )
            else:
                return AgentRollbackResponse(
                    message=f"Agent {agent_id} rollback failed: {result.error_message}",
                    agent_id=result.agent_id,
                    rolled_back_to=result.new_version,
                    rolled_back_from=result.previous_version,
                    status=result.status,
                    status_code=400
                )
                
        except Exception as e:
            self.log_error("Agent rollback failed", agent_id=agent_id, error=str(e))
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Agent rollback failed: {str(e)}"
            )

    async def get_version_history(self, agent_id: str) -> AgentVersionHistoryResponse:
        """Get version history for an agent"""
        try:
            self.log_debug("Getting version history", agent_id=agent_id)
            
            result = await self.update_service.get_version_history(agent_id)
            
            if result.get("success"):
                # Transform version data
                versions = []
                for v in result.get("versions", []):
                    version_info = AgentVersionInfo(
                        version=v.get("version", ""),
                        status=v.get("status", "unknown"),
                        created_at=v.get("created_at", ""),
                        build_ids=v.get("build_ids", []),
                        deployment_ids=v.get("deployment_ids", []),
                        git_commit=v.get("git_commit"),
                        rollback_info=v.get("rollback_info")
                    )
                    versions.append(version_info)
                
                return AgentVersionHistoryResponse(
                    agent_id=result["agent_id"],
                    current_version=result["current_version"],
                    versions=versions,
                    status_code=200,
                    message=f"Retrieved {len(versions)} versions for agent {agent_id}"
                )
            else:
                raise HTTPException(
                    status_code=status.HTTP_404_NOT_FOUND,
                    detail=result.get("error", f"Agent {agent_id} not found")
                )
                
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Failed to get version history", agent_id=agent_id, error=str(e))
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Failed to get version history: {str(e)}"
            )
