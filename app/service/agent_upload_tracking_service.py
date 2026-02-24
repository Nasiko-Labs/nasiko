import os
import time
from typing import Any
from uuid import uuid4

from app.entity.entity import UploadStatus
from app.service.agent_upload_service import AgentUploadService, AgentUploadResult
from fastapi import UploadFile
from app.pkg.config.config import settings


class AgentUploadTrackingService:
    """
    Enhanced agent upload service with comprehensive status tracking.

    This service wraps the existing AgentUploadService and adds:
    - Upload status tracking in database
    - Progress updates throughout the upload process
    - Better error handling and logging
    """

    def __init__(self, logger, repository):
        self.logger = logger
        self.repository = repository
        # Create the base upload service
        self.base_service = AgentUploadService(logger, repository)

    async def process_zip_upload(
        self, file: UploadFile, user_id: str, agent_name: str | None = None
    ) -> AgentUploadResult:
        """
        Process uploaded .zip file with comprehensive status tracking
        """
        start_time = time.time()
        upload_id = str(uuid4())

        # Get file size
        file_content = await file.read()
        file_size = len(file_content)
        await file.seek(0)  # Reset file pointer

        # Determine agent name early if possible
        temp_agent_name = agent_name or "auto-detect"

        # Create initial status record
        from datetime import datetime, timezone
        current_time = datetime.now(timezone.utc)
        
        status_data = {
            "upload_id": upload_id,
            "agent_name": temp_agent_name,
            "owner_id": user_id,
            "status": UploadStatus.INITIATED,
            "progress_percentage": 0,
            "source_info": {
                "filename": file.filename,
                "content_type": file.content_type,
            },
            "file_size": file_size,
            "status_message": "Upload initiated",
            "upload_type": "zip",
            "created_at": current_time,
            "updated_at": current_time
        }

        try:
            # Create status record in database
            if self.repository:
                await self.repository.create_upload_status(status_data)
                self.logger.info(f"Created upload status record: {upload_id}")

            # Update status: Processing
            await self._update_status(
                upload_id,
                {
                    "status": UploadStatus.PROCESSING,
                    "progress_percentage": 10,
                    "status_message": "Extracting and validating files",
                },
            )

            # Call the base service method
            result = await self.base_service.process_zip_upload(file, agent_name)
            
            # If successful, trigger orchestration with owner_id
            if result.success:
                from app.service.orchestration_service import OrchestrationService
                orchestration = OrchestrationService(self.logger)
                
                # Agents are stored versioned under /app/agents/{name}/{version} (e.g. v1.0.0).
                # If we orchestrate the unversioned dir, the BuildKit job downloads a tarball
                # that contains only the version subdir and no root-level Dockerfile.
                agent_path = f"/app/agents/{result.agent_name}"
                if getattr(result, "version", None):
                    agent_path = f"/app/agents/{result.agent_name}/{result.version}"

                orchestration_triggered = await orchestration.trigger_agent_orchestration(
                    agent_name=result.agent_name,
                    agent_path=agent_path,
                    base_url=settings.NASIKO_API_URL,
                    additional_data={
                        "owner_id": user_id,
                        "upload_id": upload_id,
                        "upload_type": "zip",
                        "version": getattr(result, "version", None),
                    }
                )
                
                # Update the result to reflect our orchestration call
                result.orchestration_triggered = orchestration_triggered

            # Update agent name if it was auto-detected
            if not agent_name and result.agent_name != temp_agent_name:
                await self._update_status(upload_id, {"agent_name": result.agent_name})

            if result.success:
                # Update progress based on what was completed
                progress = 50
                status = UploadStatus.PROCESSING
                message = "Files processed successfully"

                if result.capabilities_generated:
                    progress = 70
                    status = UploadStatus.CAPABILITIES_GENERATED
                    message = "Capabilities generated"

                if result.orchestration_triggered:
                    progress = 90
                    status = UploadStatus.ORCHESTRATION_TRIGGERED
                    message = "Orchestration triggered"

                await self._update_status(
                    upload_id,
                    {
                        "status": status,
                        "progress_percentage": progress,
                        "status_message": message,
                        "capabilities_generated": result.capabilities_generated,
                        "orchestration_triggered": result.orchestration_triggered,
                        "processing_duration": time.time() - start_time,
                    },
                )

            else:
                # Handle failure
                await self._update_status(
                    upload_id,
                    {
                        "status": UploadStatus.FAILED,
                        "progress_percentage": 0,
                        "status_message": f"Upload failed: {result.status}",
                        "error_details": [result.status],
                        "validation_errors": result.validation_errors,
                        "processing_duration": time.time() - start_time,
                    },
                )

            # Add upload_id to result
            result.upload_id = upload_id
            return result

        except Exception as e:
            self.logger.error(f"Error in zip upload tracking: {str(e)}")

            # Update status with error
            if self.repository:
                await self._update_status(
                    upload_id,
                    {
                        "status": UploadStatus.FAILED,
                        "progress_percentage": 0,
                        "status_message": f"Unexpected error: {str(e)}",
                        "error_details": [str(e)],
                        "processing_duration": time.time() - start_time,
                    },
                )

            # Return error result
            return AgentUploadResult(
                success=False,
                agent_name=temp_agent_name,
                status="error",
                validation_errors=[str(e)],
                upload_id=upload_id,
            )

    async def process_github_upload(
        self, 
        directory_path: str, 
        user_id: str, 
        agent_name: str | None = None,
        repository_full_name: str | None = None,
        branch: str | None = None
    ) -> AgentUploadResult:
        """
        Process GitHub repository upload with comprehensive status tracking
        """
        start_time = time.time()
        upload_id = str(uuid4())

        # Get directory size
        directory_size = self._calculate_directory_size(directory_path)
        temp_agent_name = agent_name or "auto-detect"

        # Create initial status record
        from datetime import datetime, timezone
        current_time = datetime.now(timezone.utc)
        
        status_data = {
            "upload_id": upload_id,
            "agent_name": temp_agent_name,
            "owner_id": user_id,
            "status": UploadStatus.INITIATED,
            "progress_percentage": 0,
            "source_info": {
                "directory_path": directory_path,
                "repository_full_name": repository_full_name,
                "branch": branch,
                "source_type": "github"
            },
            "file_size": directory_size,
            "status_message": "GitHub repository upload initiated",
            "upload_type": "github",
            "created_at": current_time,
            "updated_at": current_time
        }

        try:
            # Create status record in database
            if self.repository:
                await self.repository.create_upload_status(status_data)
                self.logger.info(f"Created GitHub upload status record: {upload_id}")

            # Update status: Processing
            await self._update_status(
                upload_id,
                {
                    "status": UploadStatus.PROCESSING,
                    "progress_percentage": 10,
                    "status_message": "Processing GitHub repository",
                },
            )

            # Call the base service method
            result = await self.base_service.process_directory_upload(
                directory_path, agent_name
            )

            if result.success:
                # Update status based on result
                await self._update_status(
                    upload_id,
                    {
                        "status": UploadStatus.CAPABILITIES_GENERATED,
                        "progress_percentage": 60,
                        "status_message": "GitHub repository processed successfully",
                        "agent_name": result.agent_name,  # Update with actual agent name
                        "capabilities_generated": result.capabilities_generated,
                    },
                )


                from app.service.orchestration_service import OrchestrationService
                orchestration = OrchestrationService(self.logger)

                agent_path = f"/app/agents/{result.agent_name}"
                if getattr(result, "version", None):
                    agent_path = f"/app/agents/{result.agent_name}/{result.version}"

                orchestration_triggered = await orchestration.trigger_agent_orchestration(
                    agent_name=result.agent_name,
                    agent_path=agent_path,
                    base_url=settings.NASIKO_API_URL,
                    additional_data={
                        "owner_id": user_id,
                        "upload_id": upload_id,
                        "upload_type": "github",
                        "repository_full_name": repository_full_name,
                        "branch": branch,
                        "version": getattr(result, "version", None),
                    }
                )

                # Update the result to reflect our orchestration call
                result.orchestration_triggered = orchestration_triggered

                if orchestration_triggered:
                    await self._update_status(
                        upload_id,
                        {
                            "status": UploadStatus.ORCHESTRATION_TRIGGERED,
                            "progress_percentage": 80,
                            "status_message": "GitHub agent deployment initiated",
                        },
                    )
                else:
                    await self._update_status(
                        upload_id,
                        {
                            "status": UploadStatus.FAILED,
                            "status_message": "Failed to trigger deployment orchestration",
                        },
                    )

                processing_time = time.time() - start_time
                self.logger.info(f"GitHub upload processing completed in {processing_time:.2f} seconds")
                return result

            else:
                # Update status: Failed
                await self._update_status(
                    upload_id,
                    {
                        "status": UploadStatus.FAILED,
                        "status_message": f"GitHub repository processing failed: {result.status}",
                        "validation_errors": result.validation_errors,
                    },
                )
                return result

        except Exception as e:
            # Update status: Failed
            await self._update_status(
                upload_id,
                {
                    "status": UploadStatus.FAILED,
                    "status_message": f"GitHub upload processing error: {str(e)}",
                },
            )
            self.logger.error(f"GitHub upload processing failed: {e}")
            raise

    async def process_directory_upload(
        self, directory_path: str, user_id: str, agent_name: str | None = None
    ) -> AgentUploadResult:
        """
        Process directory upload with comprehensive status tracking
        """
        start_time = time.time()
        upload_id = str(uuid4())

        # Get directory size
        directory_size = self._calculate_directory_size(directory_path)
        temp_agent_name = agent_name or "auto-detect"

        # Create initial status record
        from datetime import datetime, timezone
        current_time = datetime.now(timezone.utc)
        
        status_data = {
            "upload_id": upload_id,
            "agent_name": temp_agent_name,
            "owner_id": user_id,
            "status": UploadStatus.INITIATED,
            "progress_percentage": 0,
            "source_info": {
                "directory_path": directory_path,
            },
            "file_size": directory_size,
            "status_message": "Directory upload initiated",
            "upload_type": "directory",
            "created_at": current_time,
            "updated_at": current_time
        }

        try:
            # Create status record in database
            if self.repository:
                await self.repository.create_upload_status(status_data)
                self.logger.info(f"Created upload status record: {upload_id}")

            # Update status: Processing
            await self._update_status(
                upload_id,
                {
                    "status": UploadStatus.PROCESSING,
                    "progress_percentage": 10,
                    "status_message": "Validating directory structure",
                },
            )

            # Call the base service method
            result = await self.base_service.process_directory_upload(
                directory_path, agent_name
            )
            
            # If successful, trigger orchestration with owner_id
            if result.success:
                from app.service.orchestration_service import OrchestrationService
                orchestration = OrchestrationService(self.logger)
                
                agent_path = f"/app/agents/{result.agent_name}"
                if getattr(result, "version", None):
                    agent_path = f"/app/agents/{result.agent_name}/{result.version}"

                orchestration_triggered = await orchestration.trigger_agent_orchestration(
                    agent_name=result.agent_name,
                    agent_path=agent_path,
                    base_url=settings.NASIKO_API_URL,
                    additional_data={
                        "owner_id": user_id,
                        "upload_id": upload_id,
                        "upload_type": "directory",
                        "version": getattr(result, "version", None),
                    }
                )
                
                # Update the result to reflect our orchestration call
                result.orchestration_triggered = orchestration_triggered

            # Update agent name if it was auto-detected
            if not agent_name and result.agent_name != temp_agent_name:
                await self._update_status(upload_id, {"agent_name": result.agent_name})

            if result.success:
                # Update progress based on what was completed
                progress = 50
                status = UploadStatus.PROCESSING
                message = "Directory processed successfully"

                if result.capabilities_generated:
                    progress = 70
                    status = UploadStatus.CAPABILITIES_GENERATED
                    message = "Capabilities generated"

                if result.orchestration_triggered:
                    progress = 90
                    status = UploadStatus.ORCHESTRATION_TRIGGERED
                    message = "Orchestration triggered"

                await self._update_status(
                    upload_id,
                    {
                        "status": status,
                        "progress_percentage": progress,
                        "status_message": message,
                        "capabilities_generated": result.capabilities_generated,
                        "orchestration_triggered": result.orchestration_triggered,
                        "processing_duration": time.time() - start_time,
                    },
                )

            else:
                # Handle failure
                await self._update_status(
                    upload_id,
                    {
                        "status": UploadStatus.FAILED,
                        "progress_percentage": 0,
                        "status_message": f"Upload failed: {result.status}",
                        "error_details": [result.status],
                        "validation_errors": result.validation_errors,
                        "processing_duration": time.time() - start_time,
                    },
                )

            # Add upload_id to result
            result.upload_id = upload_id
            return result

        except Exception as e:
            self.logger.error(f"Error in directory upload tracking: {str(e)}")

            # Update status with error
            if self.repository:
                await self._update_status(
                    upload_id,
                    {
                        "status": UploadStatus.FAILED,
                        "progress_percentage": 0,
                        "status_message": f"Unexpected error: {str(e)}",
                        "error_details": [str(e)],
                        "processing_duration": time.time() - start_time,
                    },
                )

            # Return error result
            return AgentUploadResult(
                success=False,
                agent_name=temp_agent_name,
                status="error",
                validation_errors=[str(e)],
                upload_id=upload_id,
            )

    async def _update_status(self, upload_id: str, update_data: dict[str, Any]):
        """Update upload status in database"""
        if self.repository:
            try:
                await self.repository.update_upload_status(upload_id, update_data)
                self.logger.debug(
                    f"Updated upload status {upload_id}: {update_data.get('status', 'unknown')}"
                )
            except Exception as e:
                self.logger.error(
                    f"Failed to update upload status {upload_id}: {str(e)}"
                )

    def _calculate_directory_size(self, directory_path: str) -> int:
        """Calculate total size of directory in bytes"""
        try:
            total_size = 0
            for dirpath, dirnames, filenames in os.walk(directory_path):
                for filename in filenames:
                    file_path = os.path.join(dirpath, filename)
                    if os.path.exists(file_path):
                        total_size += os.path.getsize(file_path)
            return total_size
        except Exception as e:
            self.logger.warning(f"Could not calculate directory size: {e}")
            return 0

    async def update_upload_status_by_agent_latest(self, agent_name: str, update_data):
        """
        Update the latest upload status for an agent (wrapper for repository method)
        Used by orchestrator to update agent deployment status.
        """
        try:
            if self.repository:
                upload_data = await self.repository.update_upload_status_by_agent_name(agent_name, update_data)
                self.logger.info(f"Updated latest upload status for agent: {agent_name}")
                return upload_data
            else:
                self.logger.warning("Repository not available for upload status update")
        except Exception as e:
            self.logger.error(f"Failed to update latest upload status for agent {agent_name}: {str(e)}")
            raise
