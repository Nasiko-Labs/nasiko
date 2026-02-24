"""
Agent Upload Handler - Manages agent upload and status operations
"""

from fastapi import HTTPException, status, UploadFile
from .base_handler import BaseHandler
from ..types import (
    AgentUploadResponse, AgentUploadItemResponse, UploadStatusSingleResponse, UploadStatusUpdateRequest,
    SimpleUserUploadAgentsResponse, SimpleUserUploadAgentResponse, UploadInfoResponse
)
from typing import Optional

class AgentUploadHandler(BaseHandler):
    """Handler for agent upload and status operations"""

    def __init__(self, service, logger):
        super().__init__(service, logger)
        from app.service.agent_upload_tracking_service import AgentUploadTrackingService
        self.upload_service = AgentUploadTrackingService(logger, service.repo)
    
    def _serialize_datetime_fields(self, upload_status_dict):
        """Convert datetime objects to ISO format strings for API response"""
        if not upload_status_dict:
            return upload_status_dict
        
        # Create a copy to avoid mutating original
        serialized = dict(upload_status_dict)
        
        # Convert datetime fields to strings
        datetime_fields = ['created_at', 'updated_at', 'completed_at']
        for field in datetime_fields:
            if field in serialized and serialized[field] is not None:
                if hasattr(serialized[field], 'isoformat'):
                    serialized[field] = serialized[field].isoformat()
                
        return serialized
    
    def _serialize_datetime_fields_list(self, upload_status_list):
        """Convert datetime objects to ISO format strings for a list of upload statuses"""
        if not upload_status_list:
            return upload_status_list
            
        return [self._serialize_datetime_fields(status) for status in upload_status_list]

    async def upload_agent_directory(self, directory_path: str, user_id: str, agent_name: Optional[str] = None) -> AgentUploadResponse:
        """Upload agent from directory"""
        try:
            self.log_info("Uploading agent directory", directory_path=directory_path, user_id=user_id, agent_name=agent_name)
            # Use the upload tracking service which handles user_id
            result = await self.upload_service.process_directory_upload(directory_path, user_id, agent_name)
            
            return AgentUploadResponse(
                data=AgentUploadItemResponse(
                    success=result.success,
                    agent_name=result.agent_name,
                    status=result.status,
                    capabilities_generated=result.capabilities_generated,
                    orchestration_triggered=result.orchestration_triggered,
                    validation_errors=result.validation_errors,
                    version=result.version
                ),
                status_code=201 if result.success else 400,
                message=result.status
            )
        except Exception as e:
            await self.handle_service_error("upload_agent_directory", e)

    async def upload_agent_zip(self, file: UploadFile, user_id: str, agent_name: Optional[str] = None) -> AgentUploadResponse:
        """Upload agent from zip file"""
        try:
            self.log_info("Uploading agent zip", filename=file.filename, user_id=user_id, agent_name=agent_name)
            # Use the upload tracking service which handles user_id
            result = await self.upload_service.process_zip_upload(file, user_id, agent_name)
            
            return AgentUploadResponse(
                data=AgentUploadItemResponse(
                    success=result.success,
                    agent_name=result.agent_name,
                    status=result.status,
                    capabilities_generated=result.capabilities_generated,
                    orchestration_triggered=result.orchestration_triggered,
                    validation_errors=result.validation_errors,
                    version=result.version
                ),
                status_code=201 if result.success else 400,
                message=result.status
            )
        except Exception as e:
            await self.handle_service_error("upload_agent_zip", e)

    async def update_upload_status_by_agent_latest(self, agent_name: str, update_data: UploadStatusUpdateRequest) -> UploadStatusSingleResponse:
        """Update the latest upload status for an agent"""
        try:
            self.log_info("Updating latest upload status for agent", agent_name=agent_name)
            upload_status = await self.upload_service.update_upload_status_by_agent_latest(agent_name, update_data)
            if upload_status:
                # Convert datetime fields to strings for API response
                serialized_status = self._serialize_datetime_fields(upload_status)
                return UploadStatusSingleResponse(
                    data=serialized_status,
                    status_code=200,
                    message=f"Latest upload status for agent {agent_name} updated successfully"
                )
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail=f"No upload status found for agent {agent_name}"
            )
        except ValueError as e:
            self.log_error("Upload status update failed - validation error", e)
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=str(e)
            )
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("update_upload_status_by_agent_latest", e)

    async def update_upload_status(self, upload_id: str, update_data: UploadStatusUpdateRequest) -> UploadStatusSingleResponse:
        """Update upload status"""
        try:
            self.log_info("Updating upload status", upload_id=upload_id)
            upload_status = await self.upload_service.update_upload_status(upload_id, update_data)
            if upload_status:
                # Convert datetime fields to strings for API response
                serialized_status = self._serialize_datetime_fields(upload_status)
                return UploadStatusSingleResponse(
                    data=serialized_status,
                    status_code=200,
                    message="Upload status updated successfully"
                )
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail=f"Upload status with id {upload_id} not found"
            )
        except ValueError as e:
            self.log_error("Upload status update failed - validation error", e)
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=str(e)
            )
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("update_upload_status", e)

    async def get_user_upload_agents(self, user_id: str, limit: int = 100) -> SimpleUserUploadAgentsResponse:
        """Get all agents uploaded by a user with simplified format"""
        try:
            self.log_info("Fetching user upload agents", user_id=user_id, limit=limit)
            
            # Get upload statuses for the user
            upload_statuses = await self.service.get_upload_statuses_by_user(user_id, limit)
            
            simple_agents = []
            
            for status in upload_statuses:
                agent_name = status.get("agent_name", "")
                upload_type = status.get("upload_type", "unknown")
                status_state = self._map_status_to_state(status.get("status", ""))
                
                # Try to get registry information
                description = None
                url = None
                tags = []
                skills = []
                agent_id = ""

                try:
                    registry_entry = await self.service.get_registry_by_agent_id(agent_name)
                    if registry_entry:
                        # Get agent_id from registry
                        if hasattr(registry_entry, 'id'):
                            agent_id = registry_entry.id

                        # Get the actual agent name (workflow name for N8N)
                        if hasattr(registry_entry, 'name'):
                            # Update agent_name to use the correct name from registry
                            agent_name = registry_entry.name

                        # Get description
                        if hasattr(registry_entry, 'description'):
                            description = registry_entry.description

                        # Get url from registry
                        if hasattr(registry_entry, 'url'):
                            url = registry_entry.url

                        # Get tags
                        if hasattr(registry_entry, 'tags') and registry_entry.tags:
                            tags = registry_entry.tags

                        # Get skills
                        if hasattr(registry_entry, 'skills') and registry_entry.skills:
                            skills = [
                                skill.model_dump() if hasattr(skill, 'model_dump') else skill
                                for skill in registry_entry.skills
                            ]
                except:
                    # Continue if registry entry not found
                    pass
                
                # Set default description if not found in registry
                if not description:
                    if status_state == "Setting Up":
                        description = f"Agent is being set up from {upload_type}. Setup in progress..."
                    elif status_state == "Failed":
                        description = f"Agent setup failed. Please check the upload details."
                    else:
                        description = f"Agent successfully deployed and ready to use."
                
                # Create simplified response item
                simple_agent = SimpleUserUploadAgentResponse(
                    agent_id=agent_id,  # id field from registry collection
                    agent_name=agent_name,  # name field from database
                    url=url,
                    upload_info=UploadInfoResponse(
                        upload_type=upload_type,
                        upload_status=status_state
                    ),
                    tags=tags,
                    description=description,
                    skills=skills
                )
                simple_agents.append(simple_agent)
            
            return SimpleUserUploadAgentsResponse(
                data=simple_agents,
                status_code=200,
                message=f"Retrieved {len(simple_agents)} upload agents for user"
            )
            
        except Exception as e:
            await self.handle_service_error("get_user_upload_agents", e)
    
    def _map_status_to_state(self, status: str) -> str:
        """Map detailed status to user-friendly status state"""
        status_lower = status.lower()

        # Failed states
        if any(failed_term in status_lower for failed_term in ["failed", "error", "cancelled"]):
            return "Failed"

        # Active states (completed/deployed)
        if any(active_term in status_lower for active_term in ["completed", "deployed", "active", "running"]):
            return "Active"

        # Setting Up states (all other processing states)
        return "Setting Up"

    async def download_agent_files(self, agent_name: str, version: Optional[str] = None):
        """
        Download agent files as a tarball for BuildKit to use.
        This endpoint serves the agent directory as a compressed tarball.
        If version is provided, serves from /app/agents/{agent_name}/v{version}
        Otherwise serves from /app/agents/{agent_name} (for backward compatibility)
        """
        import os
        import tarfile
        import tempfile
        from pathlib import Path
        from fastapi.responses import FileResponse

        try:
            # Determine agent path based on version
            if version:
                agent_path = Path(f"/app/agents/{agent_name}/v{version}")
                self.log_info(f"Downloading versioned agent files for '{agent_name}' version '{version}'")
            else:
                agent_path = Path(f"/app/agents/{agent_name}")
                self.log_info(f"Downloading agent files for '{agent_name}' (no version specified)")

            if not agent_path.exists() or not agent_path.is_dir():
                error_msg = f"Agent '{agent_name}'"
                if version:
                    error_msg += f" version '{version}'"
                error_msg += " not found"
                
                raise HTTPException(
                    status_code=status.HTTP_404_NOT_FOUND,
                    detail=error_msg
                )

            # Create a temporary tarball
            with tempfile.NamedTemporaryFile(mode='w+b', suffix='.tar.gz', delete=False) as tmp_file:
                tar_path = tmp_file.name

                # Create tarball of the agent directory
                with tarfile.open(tar_path, "w:gz") as tar:
                    # Add all files from the agent directory to the tarball
                    # Using arcname="" to extract files directly to workspace root
                    for item in agent_path.iterdir():
                        tar.add(item, arcname=item.name)

                # Log with version info
                version_info = f" version '{version}'" if version else ""
                self.log_info(f"Created tarball for agent '{agent_name}'{version_info} at {tar_path}")

            # Return the tarball as a file response
            filename = f"{agent_name}"
            if version:
                filename += f"-v{version}"
            filename += ".tar.gz"
            
            return FileResponse(
                path=tar_path,
                media_type="application/gzip",
                filename=filename,
                background=None  # File will be deleted by FastAPI after sending
            )

        except HTTPException:
            raise
        except Exception as e:
            self.log_error(f"Error creating agent tarball for '{agent_name}'", e)
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Failed to create agent tarball: {str(e)}"
            )