"""
Agent Update Service - Handles agent updates, versioning, and rollbacks
"""
import os
import time
import semver
from typing import Optional, Dict, Any, List
from uuid import uuid4
from datetime import datetime, timezone
from fastapi import UploadFile

from app.entity.entity import UploadStatus
from app.service.agent_upload_service import AgentUploadService, AgentUploadResult
from app.service.orchestration_service import OrchestrationService


class AgentUpdateResult:
    """Result of an agent update operation"""
    
    def __init__(
        self,
        success: bool,
        agent_id: str,
        new_version: str,
        previous_version: Optional[str] = None,
        build_id: Optional[str] = None,
        deployment_id: Optional[str] = None,
        update_strategy: str = "rolling",
        status: str = "initiated",
        error_message: Optional[str] = None,
        upload_id: Optional[str] = None
    ):
        self.success = success
        self.agent_id = agent_id
        self.new_version = new_version
        self.previous_version = previous_version
        self.build_id = build_id
        self.deployment_id = deployment_id
        self.update_strategy = update_strategy
        self.status = status
        self.error_message = error_message
        self.upload_id = upload_id


class AgentUpdateService:
    """Service for handling agent updates with version management"""
    
    def __init__(self, logger, repository):
        self.logger = logger
        self.repository = repository
        # Use existing upload service for file processing
        self.upload_service = AgentUploadService(logger, repository)
        self.orchestration_service = OrchestrationService(logger)
        # Import agentcard service for version validation
        from app.service.agentcard_service import AgentCardService
        self.agentcard_service = AgentCardService(logger)
    
    async def update_agent(
        self, 
        agent_id: str, 
        file: UploadFile, 
        user_id: str,
        version: str = "auto",
        update_strategy: str = "rolling",
        cleanup_old: bool = True,
        description: Optional[str] = None
    ) -> AgentUpdateResult:
        """
        Update an existing agent with new code
        
        Args:
            agent_id: ID of the agent to update
            file: New agent code zip file (optional for GitHub-sourced agents)
            user_id: ID of the user performing the update
            version: Version strategy ("auto", "major", "minor", "patch", or specific version)
            update_strategy: Deployment strategy ("rolling" or "blue-green") 
            cleanup_old: Whether to cleanup old deployments
            description: Optional description of the update
        """
        start_time = time.time()
        upload_id = str(uuid4())
        
        try:
            # 1. Validate agent exists and get current info
            registry_entry = await self.repository.get_registry_by_agent_id(agent_id)
            if not registry_entry:
                return AgentUpdateResult(
                    success=False,
                    agent_id=agent_id,
                    new_version="unknown",
                    status="failed",
                    error_message=f"Agent {agent_id} not found in registry"
                )
            
            # 2. Calculate new version
            current_version = registry_entry.get("version", "1.0.0")
            new_version = self._calculate_new_version(current_version, version)
            
            self.logger.info(f"AGENT_UPDATE: Updating agent {agent_id} from {current_version} to {new_version}")
            
            # 3. Check if this is a GitHub-sourced agent and handle accordingly
            if not file:
                # No file provided - check if this is a GitHub-sourced agent
                original_source = await self._get_agent_original_source(agent_id)
                if original_source and original_source.get("upload_type") == "github":
                    return await self._handle_github_agent_update(
                        agent_id, user_id, new_version, current_version, 
                        update_strategy, cleanup_old, description, 
                        upload_id, original_source
                    )
                else:
                    return AgentUpdateResult(
                        success=False,
                        agent_id=agent_id,
                        new_version=new_version,
                        previous_version=current_version,
                        status="failed",
                        error_message="No file provided for update and agent was not originally sourced from GitHub",
                        upload_id=upload_id
                    )
            
            # 4. Find and update existing upload status record (instead of creating new one)
            existing_upload_id = await self._find_existing_upload_record(agent_id)
            if existing_upload_id:
                upload_id = existing_upload_id
                self.logger.info(f"AGENT_UPDATE: Reusing existing upload record: {upload_id}")
                await self._add_update_to_existing_record(
                    upload_id, current_version, new_version, update_strategy, file.filename if file else "github-update"
                )
            else:
                # Fallback: Create new record if no existing record found
                await self._create_update_status_record(
                    upload_id, agent_id, user_id, current_version, new_version, 
                    update_strategy, file.filename if file else "github-update"
                )
            
            # 5. Process the uploaded file using existing upload service
            # We'll save it to a versioned directory
            agent_name = registry_entry.get("id", agent_id)
            
            # Update status: Processing files
            await self._update_status(upload_id, {
                "status": UploadStatus.PROCESSING,
                "progress_percentage": 20,
                "status_message": "Processing uploaded files"
            })
            
            # Use the base upload service but with versioned storage
            upload_result = await self._process_versioned_upload(file, agent_name, new_version)
            
            if not upload_result.success:
                await self._update_status(upload_id, {
                    "status": UploadStatus.FAILED,
                    "status_message": f"File processing failed: {upload_result.status}",
                    "error_details": upload_result.validation_errors
                })
                return AgentUpdateResult(
                    success=False,
                    agent_id=agent_id,
                    new_version=new_version,
                    previous_version=current_version,
                    status="failed",
                    error_message=upload_result.status,
                    upload_id=upload_id
                )
            
            # 5. Update registry with new version info
            await self._update_registry_version(agent_id, new_version, current_version, description)
            
            # 6. Update status: Triggering orchestration
            await self._update_status(upload_id, {
                "status": UploadStatus.ORCHESTRATION_TRIGGERED,
                "progress_percentage": 80,
                "status_message": "Triggering build and deployment"
            })
            
            # 7. Trigger orchestration with update action
            orchestration_triggered = await self.orchestration_service.trigger_agent_orchestration(
                agent_name=agent_name,
                agent_path=f"/app/agents/{agent_name}/v{new_version}",
                base_url="http://nasiko-backend.nasiko.svc.cluster.local:8000",
                additional_data={
                    "action": "update_agent",  # NEW: Different action for updates
                    "agent_id": agent_id,
                    "new_version": new_version,
                    "previous_version": current_version,
                    "update_strategy": update_strategy,
                    "cleanup_old": str(cleanup_old),
                    "owner_id": user_id,
                    "upload_id": upload_id,
                    "upload_type": "agent_update",
                    "description": description or ""
                }
            )
            
            if orchestration_triggered:
                await self._update_status(upload_id, {
                    "status": UploadStatus.ORCHESTRATION_PROCESSING,
                    "progress_percentage": 90,
                    "status_message": f"Building and deploying version {new_version}"
                })
                
                return AgentUpdateResult(
                    success=True,
                    agent_id=agent_id,
                    new_version=new_version,
                    previous_version=current_version,
                    update_strategy=update_strategy,
                    status="building",
                    upload_id=upload_id
                )
            else:
                await self._update_status(upload_id, {
                    "status": UploadStatus.FAILED,
                    "status_message": "Failed to trigger orchestration"
                })
                return AgentUpdateResult(
                    success=False,
                    agent_id=agent_id,
                    new_version=new_version,
                    previous_version=current_version,
                    status="failed",
                    error_message="Failed to trigger orchestration",
                    upload_id=upload_id
                )
                
        except Exception as e:
            self.logger.error(f"AGENT_UPDATE: Update failed for {agent_id}: {str(e)}")
            await self._update_status(upload_id, {
                "status": UploadStatus.FAILED,
                "status_message": f"Update failed: {str(e)}"
            })
            return AgentUpdateResult(
                success=False,
                agent_id=agent_id,
                new_version=new_version if 'new_version' in locals() else "unknown",
                status="failed",
                error_message=str(e),
                upload_id=upload_id
            )
    
    async def rollback_agent(
        self, 
        agent_id: str, 
        user_id: str,
        target_version: Optional[str] = None,
        cleanup_failed: bool = True,
        reason: Optional[str] = None
    ) -> AgentUpdateResult:
        """
        Rollback an agent to a previous version
        """
        try:
            # 1. Get agent registry info
            registry_entry = await self.repository.get_registry_by_agent_id(agent_id)
            if not registry_entry:
                return AgentUpdateResult(
                    success=False,
                    agent_id=agent_id,
                    new_version="unknown",
                    status="failed",
                    error_message=f"Agent {agent_id} not found"
                )
            
            current_version = registry_entry.get("version", "1.0.0")
            version_history = registry_entry.get("version_history", [])
            
            # 2. Determine target version
            if not target_version:
                # Find the most recent previous version
                active_versions = [v for v in version_history if v.get("status") == "active" and v.get("version") != current_version]
                if not active_versions:
                    return AgentUpdateResult(
                        success=False,
                        agent_id=agent_id,
                        new_version=current_version,
                        status="failed",
                        error_message="No previous version available for rollback"
                    )
                target_version = sorted(active_versions, key=lambda x: x.get("created_at", ""), reverse=True)[0]["version"]
            
            # 3. Trigger rollback orchestration
            agent_name = registry_entry.get("name", agent_id)
            orchestration_triggered = await self.orchestration_service.trigger_agent_orchestration(
                agent_name=agent_name,
                agent_path=f"/app/agents/{agent_name}/v{target_version}",
                base_url="http://nasiko-backend.nasiko.svc.cluster.local:8000",
                additional_data={
                    "action": "rollback_agent",  # NEW: Rollback action
                    "agent_id": agent_id,
                    "target_version": target_version,
                    "current_version": current_version,
                    "cleanup_failed": str(cleanup_failed),
                    "owner_id": user_id,
                    "upload_type": "agent_rollback",
                    "reason": reason or ""
                }
            )
            
            if orchestration_triggered:
                # 4. Update registry to mark rollback in progress
                await self._update_registry_rollback(agent_id, target_version, current_version, reason)
                
                return AgentUpdateResult(
                    success=True,
                    agent_id=agent_id,
                    new_version=target_version,
                    previous_version=current_version,
                    status="rolling_back"
                )
            else:
                return AgentUpdateResult(
                    success=False,
                    agent_id=agent_id,
                    new_version=target_version,
                    previous_version=current_version,
                    status="failed",
                    error_message="Failed to trigger rollback orchestration"
                )
                
        except Exception as e:
            self.logger.error(f"AGENT_UPDATE: Rollback failed for {agent_id}: {str(e)}")
            return AgentUpdateResult(
                success=False,
                agent_id=agent_id,
                new_version=target_version if 'target_version' in locals() else "unknown",
                status="failed",
                error_message=str(e)
            )
    
    async def get_version_history(self, agent_id: str) -> Dict[str, Any]:
        """Get version history for an agent"""
        try:
            registry_entry = await self.repository.get_registry_by_agent_id(agent_id)
            if not registry_entry:
                return {
                    "success": False,
                    "error": f"Agent {agent_id} not found"
                }
            
            current_version = registry_entry.get("version", "1.0.0")
            version_history = registry_entry.get("version_history", [])
            
            return {
                "success": True,
                "agent_id": agent_id,
                "current_version": current_version,
                "versions": version_history
            }
            
        except Exception as e:
            self.logger.error(f"AGENT_UPDATE: Failed to get version history for {agent_id}: {str(e)}")
            return {
                "success": False,
                "error": str(e)
            }
    
    def _calculate_new_version(self, current_version: str, version_strategy: str) -> str:
        """Calculate the new version based on strategy"""
        try:
            # Normalize version if needed
            if not current_version.startswith('v'):
                if len(current_version.split('.')) < 3:
                    current_version = "1.0.0"
            else:
                current_version = current_version[1:]  # Remove 'v' prefix
            
            if version_strategy == "auto" or version_strategy == "patch":
                return semver.bump_patch(current_version)
            elif version_strategy == "minor":
                return semver.bump_minor(current_version)
            elif version_strategy == "major":
                return semver.bump_major(current_version)
            else:
                # Specific version provided
                return semver.VersionInfo.parse(version_strategy).strip()
                
        except Exception as e:
            self.logger.warning(f"AGENT_UPDATE: Version calculation failed, using semantic fallback: {e}")
            # Fallback to semantic versioning with patch increment
            # This maintains valid semantic versioning instead of creating invalid versions
            try:
                # Try to parse current version and increment patch
                if current_version.startswith('v'):
                    clean_version = current_version[1:]
                else:
                    clean_version = current_version
                    
                # If version is invalid, start from 1.0.0
                if len(clean_version.split('.')) != 3:
                    return "1.0.1"  # Start with 1.0.1 as fallback
                
                return semver.bump_patch(clean_version)
            except:
                # Ultimate fallback - use 1.0.1 instead of timestamp
                self.logger.warning(f"AGENT_UPDATE: Ultimate fallback used - returning 1.0.1")
                return "1.0.1"
    
    async def _process_versioned_upload(self, file: UploadFile, agent_name: str, version: str) -> AgentUploadResult:
        """Process uploaded file and save directly to versioned directory"""
        import shutil
        import tempfile
        
        try:
            # Create versioned directory for the new version
            versioned_path = f"/app/agents/{agent_name}/v{version}"
            os.makedirs(versioned_path, exist_ok=True)
            self.logger.info(f"AGENT_UPDATE: Created versioned directory: {versioned_path}")
            
            # Use a temporary directory for processing
            with tempfile.TemporaryDirectory() as temp_dir:
                # Process the upload in temp directory first
                temp_agent_name = f"temp_{agent_name}_{int(time.time())}"
                result = await self.upload_service.process_zip_upload(file, temp_agent_name)
                
                if result.success:
                    # Copy processed files directly to versioned directory
                    temp_agent_path = f"/app/agents/{temp_agent_name}"
                    
                    if os.path.exists(temp_agent_path):
                        self.logger.info(f"AGENT_UPDATE: Copying processed files to {versioned_path}")
                        
                        # The upload service may create a versioned folder in temp, we need to copy from inside that
                        # to avoid nested version folders (v1.0.1/v1.0.0/)
                        
                        # Check for any versioned subdirectory (v1.0.0, v1.0.1, etc.)
                        versioned_subdirs = [d for d in os.listdir(temp_agent_path) 
                                           if os.path.isdir(os.path.join(temp_agent_path, d)) and d.startswith('v')]
                        
                        if versioned_subdirs:
                            # Use the first versioned subdirectory found
                            temp_version_subdir = versioned_subdirs[0]
                            source_path = os.path.join(temp_agent_path, temp_version_subdir)
                            self.logger.info(f"AGENT_UPDATE: Found versioned subdirectory {temp_version_subdir}, copying from: {source_path}")
                        else:
                            # No versioned subdirectory found, copy from temp root
                            source_path = temp_agent_path
                            self.logger.info(f"AGENT_UPDATE: No versioned subdirectory found, copying from temp root: {source_path}")
                        
                        # Copy all files and directories from source to versioned directory
                        for item in os.listdir(source_path):
                            src_path = os.path.join(source_path, item)
                            dst_path = os.path.join(versioned_path, item)
                            
                            try:
                                if os.path.isdir(src_path):
                                    if os.path.exists(dst_path):
                                        shutil.rmtree(dst_path)
                                    shutil.copytree(src_path, dst_path)
                                else:
                                    shutil.copy2(src_path, dst_path)
                                self.logger.debug(f"AGENT_UPDATE: Copied {item} to versioned directory")
                            except Exception as copy_error:
                                self.logger.error(f"AGENT_UPDATE: Failed to copy {item}: {copy_error}")
                                raise copy_error
                        
                        # Ensure AgentCard.json exists in versioned directory (consistency with upload flow)
                        await self.upload_service._ensure_agentcard_json(versioned_path, agent_name)
                        self.logger.info(f"AGENT_UPDATE: Ensured AgentCard.json exists in {versioned_path}")
                        
                        # Validate AgentCard version against expected version
                        validation_result = await self._validate_agentcard_version(versioned_path, version, agent_name)
                        if not validation_result["valid"]:
                            self.logger.warning(f"AGENT_UPDATE: {validation_result['message']}")
                            # Log warning but continue - don't fail the update for version mismatches
                            # This preserves backward compatibility while highlighting potential issues
                        
                        # Clean up temp directory
                        shutil.rmtree(temp_agent_path)
                        self.logger.info(f"AGENT_UPDATE: Successfully created version {version} in {versioned_path}")
                        
                        # Update result to reflect versioned path
                        result.agent_name = agent_name
                        result.agent_path = versioned_path
                    else:
                        self.logger.error(f"AGENT_UPDATE: Temp agent path {temp_agent_path} not found after processing")
                        result.success = False
                        result.status = "Temp agent directory not found after processing"
                
                return result
                    
        except Exception as e:
            self.logger.error(f"AGENT_UPDATE: Versioned upload processing failed: {e}")
            return AgentUploadResult(
                success=False,
                agent_name=agent_name,
                status=f"Versioned upload failed: {str(e)}",
                capabilities_generated=False,
                orchestration_triggered=False
            )
    
    async def _process_versioned_github_upload(self, temp_dir: str, agent_name: str, version: str) -> AgentUploadResult:
        """Process GitHub cloned directory and save directly to versioned directory - same pattern as ZIP flow"""
        import shutil
        
        try:
            # Create versioned directory for the new version
            versioned_path = f"/app/agents/{agent_name}/v{version}"
            os.makedirs(versioned_path, exist_ok=True)
            self.logger.info(f"AGENT_UPDATE: Created versioned directory: {versioned_path}")
            
            # Process the GitHub directory in temp using upload service (same as ZIP flow)
            temp_agent_name = f"temp_{agent_name}_{int(time.time())}"
            result = await self.upload_service.process_directory_upload(temp_dir, temp_agent_name)
            
            if result.success:
                # Copy processed files directly to versioned directory (same as ZIP flow)
                temp_agent_path = f"/app/agents/{temp_agent_name}"
                
                if os.path.exists(temp_agent_path):
                    self.logger.info(f"AGENT_UPDATE: Copying processed files to {versioned_path}")
                    
                    # The upload service may create a versioned folder in temp, we need to copy from inside that
                    # to avoid nested version folders (v1.0.1/v1.0.0/)
                    
                    # Check for any versioned subdirectory (v1.0.0, v1.0.1, etc.)
                    versioned_subdirs = [d for d in os.listdir(temp_agent_path) 
                                       if os.path.isdir(os.path.join(temp_agent_path, d)) and d.startswith('v')]
                    
                    if versioned_subdirs:
                        # Use the first versioned subdirectory found
                        temp_version_subdir = versioned_subdirs[0]
                        source_path = os.path.join(temp_agent_path, temp_version_subdir)
                        self.logger.info(f"AGENT_UPDATE: Found versioned subdirectory {temp_version_subdir}, copying from: {source_path}")
                    else:
                        # No versioned subdirectory found, copy from temp root
                        source_path = temp_agent_path
                        self.logger.info(f"AGENT_UPDATE: No versioned subdirectory found, copying from temp root: {source_path}")
                    
                    # Copy all files and directories from source to versioned directory
                    for item in os.listdir(source_path):
                        src_path = os.path.join(source_path, item)
                        dst_path = os.path.join(versioned_path, item)
                        
                        try:
                            if os.path.isdir(src_path):
                                if os.path.exists(dst_path):
                                    shutil.rmtree(dst_path)
                                shutil.copytree(src_path, dst_path)
                            else:
                                shutil.copy2(src_path, dst_path)
                            self.logger.debug(f"AGENT_UPDATE: Copied {item} to versioned directory")
                        except Exception as copy_error:
                            self.logger.error(f"AGENT_UPDATE: Failed to copy {item}: {copy_error}")
                            raise copy_error
                    
                    # Ensure AgentCard.json exists in versioned directory (consistency with upload flow)
                    await self.upload_service._ensure_agentcard_json(versioned_path, agent_name)
                    self.logger.info(f"AGENT_UPDATE: Ensured AgentCard.json exists in {versioned_path}")
                    
                    # Clean up temp directory
                    shutil.rmtree(temp_agent_path)
                    self.logger.info(f"AGENT_UPDATE: Successfully created GitHub version {version} in {versioned_path}")
                    
                    # Update result to reflect versioned path
                    result.agent_name = agent_name
                    result.agent_path = versioned_path
                else:
                    self.logger.error(f"AGENT_UPDATE: Temp agent path {temp_agent_path} not found after processing")
                    result.success = False
                    result.status = "Temp agent directory not found after processing"
            
            return result
                
        except Exception as e:
            self.logger.error(f"AGENT_UPDATE: Versioned GitHub upload processing failed: {e}")
            return AgentUploadResult(
                success=False,
                agent_name=agent_name,
                status=f"Versioned GitHub upload failed: {str(e)}",
                capabilities_generated=False,
                orchestration_triggered=False
            )
    
    async def _find_existing_upload_record(self, agent_id: str) -> Optional[str]:
        """Find existing upload record for the agent"""
        try:
            upload_records = await self.repository.get_upload_status_by_agent_name(agent_id)
            if upload_records:
                # Return the most recent successful upload record
                for record in upload_records:
                    if record.get("status") in [UploadStatus.COMPLETED, UploadStatus.ORCHESTRATION_PROCESSING]:
                        return record.get("upload_id")
            return None
        except Exception as e:
            self.logger.error(f"AGENT_UPDATE: Error finding existing upload record: {e}")
            return None
    
    async def _add_update_to_existing_record(
        self, upload_id: str, current_version: str, new_version: str, 
        update_strategy: str, filename: str
    ):
        """Add update info to existing upload record"""
        try:
            current_time = datetime.now(timezone.utc)
            
            # Get existing record
            existing_record = await self.repository.get_upload_status_by_upload_id(upload_id)
            if not existing_record:
                self.logger.error(f"AGENT_UPDATE: Upload record {upload_id} not found")
                return
            
            # Initialize upload_history if it doesn't exist
            upload_history = existing_record.get("upload_history", [])
            
            # Add the new update to history
            new_update = {
                "version": new_version,
                "status": "initiated", 
                "created_at": current_time.isoformat(),
                "upload_type": "agent_update",
                "filename": filename,
                "update_strategy": update_strategy,
                "previous_version": current_version
            }
            upload_history.append(new_update)
            
            # Update the record
            update_data = {
                "current_version": new_version,
                "upload_history": upload_history,
                "status": UploadStatus.INITIATED,
                "progress_percentage": 0,
                "status_message": f"Agent update initiated: {current_version} → {new_version}",
                "updated_at": current_time
            }
            
            await self.repository.update_upload_status(upload_id, update_data)
            self.logger.info(f"AGENT_UPDATE: Added update to existing record: {upload_id}")
            
        except Exception as e:
            self.logger.error(f"AGENT_UPDATE: Error adding update to record: {e}")

    async def _create_update_status_record(
        self, upload_id: str, agent_id: str, user_id: str, 
        current_version: str, new_version: str, update_strategy: str, filename: str
    ):
        """Create initial status record for the update"""
        current_time = datetime.now(timezone.utc)
        
        status_data = {
            "upload_id": upload_id,
            "agent_name": agent_id,
            "owner_id": user_id,
            "status": UploadStatus.INITIATED,
            "progress_percentage": 0,
            "current_version": new_version,
            "upload_history": [
                {
                    "version": new_version,
                    "status": "initiated",
                    "created_at": current_time.isoformat(),
                    "upload_type": "agent_update",
                    "filename": filename,
                    "update_strategy": update_strategy,
                    "previous_version": current_version
                }
            ],
            "source_info": {
                "filename": filename,
                "update_type": "agent_update",
                "current_version": current_version,
                "new_version": new_version,
                "update_strategy": update_strategy
            },
            "status_message": f"Agent update initiated: {current_version} → {new_version}",
            "upload_type": "agent_update",
            "created_at": current_time,
            "updated_at": current_time
        }
        
        if self.repository:
            await self.repository.create_upload_status(status_data)
            self.logger.info(f"AGENT_UPDATE: Created update status record: {upload_id}")
    
    async def _update_status(self, upload_id: str, update_data: Dict[str, Any]):
        """Update the status record"""
        try:
            if self.repository:
                update_data["updated_at"] = datetime.now(timezone.utc)
                await self.repository.update_upload_status(upload_id, update_data)
        except Exception as e:
            self.logger.error(f"AGENT_UPDATE: Failed to update status {upload_id}: {e}")
    
    async def _update_registry_version(
        self, agent_id: str, new_version: str, previous_version: str, description: Optional[str]
    ):
        """Update registry with new version information"""
        try:
            current_time = datetime.now(timezone.utc)
            
            # Get current registry entry
            registry_entry = await self.repository.get_registry_by_agent_id(agent_id)
            if not registry_entry:
                self.logger.error(f"AGENT_UPDATE: Registry entry not found for agent_id: {agent_id}")
                return
            
            # Update version and add to history
            version_history = registry_entry.get("version_history", [])
            
            # Mark previous version as archived
            for v in version_history:
                if v.get("version") == previous_version:
                    v["status"] = "archived"
            
            # Add new version to history
            new_version_info = {
                "version": new_version,
                "status": "building",  # Will be updated to "active" when deployment succeeds
                "created_at": current_time.isoformat(),
                "build_ids": [],
                "deployment_ids": [],
                "description": description,
                "rollback_info": {
                    "can_rollback": True,
                    "previous_version": previous_version
                }
            }
            
            version_history.append(new_version_info)
            
            # Update registry
            update_data = {
                "version": new_version,
                "version_history": version_history,
                "updated_at": current_time
            }
            
            from bson import ObjectId
            await self.repository.update_registry(ObjectId(registry_entry["_id"]), update_data)
            self.logger.info(f"AGENT_UPDATE: Updated registry version for {agent_id}: {previous_version} → {new_version}")
            
        except Exception as e:
            self.logger.error(f"AGENT_UPDATE: Failed to update registry version: {e}")
    
    async def _update_registry_rollback(
        self, agent_id: str, target_version: str, current_version: str, reason: Optional[str]
    ):
        """Update registry for rollback operation"""
        try:
            registry_entry = await self.repository.get_registry_by_agent_id(agent_id)
            if not registry_entry:
                return
            
            version_history = registry_entry.get("version_history", [])
            
            # Mark current version as failed/archived
            for v in version_history:
                if v.get("version") == current_version:
                    v["status"] = "failed"
                    if reason:
                        v["rollback_reason"] = reason
            
            # Mark target version as active
            for v in version_history:
                if v.get("version") == target_version:
                    v["status"] = "active"
            
            # Update registry
            update_data = {
                "version": target_version,
                "version_history": version_history,
                "updated_at": datetime.now(timezone.utc)
            }
            
            from bson import ObjectId
            await self.repository.update_registry(ObjectId(registry_entry["_id"]), update_data)
            self.logger.info(f"AGENT_UPDATE: Updated registry for rollback {agent_id}: {current_version} → {target_version}")
            
        except Exception as e:
            self.logger.error(f"AGENT_UPDATE: Failed to update registry for rollback: {e}")
    
    async def _get_agent_original_source(self, agent_id: str) -> Optional[Dict[str, Any]]:
        """Get the original source information for an agent from upload status"""
        try:
            # Find the most recent successful upload for this agent
            upload_records = await self.repository.get_upload_status_by_agent_name(agent_id)
            
            if upload_records:
                # Look for the most recent successful upload
                for record in upload_records:
                    if record.get("status") in [UploadStatus.COMPLETED, UploadStatus.ORCHESTRATION_PROCESSING]:
                        return record
            
            self.logger.info(f"AGENT_UPDATE: No original source found for agent {agent_id}")
            return None
            
        except Exception as e:
            self.logger.error(f"AGENT_UPDATE: Error getting original source for {agent_id}: {e}")
            return None
    
    async def _handle_github_agent_update(
        self, 
        agent_id: str, 
        user_id: str, 
        new_version: str, 
        current_version: str,
        update_strategy: str, 
        cleanup_old: bool, 
        description: Optional[str],
        upload_id: str,
        original_source: Dict[str, Any]
    ) -> AgentUpdateResult:
        """Handle updates for GitHub-sourced agents"""
        try:
            self.logger.info(f"AGENT_UPDATE: Handling GitHub agent update for {agent_id}")
            
            # Get agent registry entry for agent_name (same as ZIP flow)
            registry_entry = await self.repository.get_registry_by_agent_id(agent_id)
            if not registry_entry:
                return AgentUpdateResult(
                    success=False,
                    agent_id=agent_id,
                    new_version=new_version,
                    previous_version=current_version,
                    status="failed",
                    error_message=f"Agent {agent_id} not found in registry",
                    upload_id=upload_id
                )
            
            agent_name = registry_entry.get("id", agent_id)
            
            # Extract GitHub information from original source
            source_info = original_source.get("source_info", {})
            repository_full_name = source_info.get("repository_full_name")
            branch = source_info.get("branch", "main")
            
            if not repository_full_name:
                return AgentUpdateResult(
                    success=False,
                    agent_id=agent_id,
                    new_version=new_version,
                    previous_version=current_version,
                    status="failed",
                    error_message="GitHub repository information not found in original upload",
                    upload_id=upload_id
                )
            
            # Find and update existing upload status record (same logic as ZIP updates)
            existing_upload_id = await self._find_existing_upload_record(agent_id)
            if existing_upload_id:
                upload_id = existing_upload_id
                self.logger.info(f"AGENT_UPDATE: Reusing existing upload record for GitHub update: {upload_id}")
                await self._add_update_to_existing_record(
                    upload_id, current_version, new_version, update_strategy, "github-update"
                )
            else:
                # Fallback: Create new record if no existing record found
                await self._create_update_status_record(
                    upload_id, agent_id, user_id, current_version, new_version, 
                    update_strategy, "github-update"
                )
            
            # Update status: Cloning from GitHub
            await self._update_status(upload_id, {
                "status": UploadStatus.PROCESSING,
                "progress_percentage": 20,
                "status_message": f"Cloning latest code from GitHub: {repository_full_name}@{branch}"
            })
            
            # Clone and process GitHub repository directly (without creating new upload record)
            from app.service.github_service import GitHubService
            github_service = GitHubService(self.repository, self.logger)
            
            # Get user's GitHub credentials
            credential = await self.repository.get_user_github_credential_decrypted(user_id)
            if not credential or not credential.get('access_token'):
                return AgentUpdateResult(
                    success=False,
                    agent_id=agent_id,
                    new_version=new_version,
                    previous_version=current_version,
                    status="failed",
                    error_message="No valid GitHub credentials found",
                    upload_id=upload_id
                )
            
            access_token = credential['access_token']
            
            # Clone repository to temporary directory
            temp_dir = await github_service._clone_repository(
                repository_full_name, branch, access_token
            )
            
            try:
                # Process the cloned repository using existing upload service
                upload_result = await self._process_versioned_github_upload(temp_dir, agent_name, new_version)
            finally:
                # Clean up temporary directory
                if temp_dir and os.path.exists(temp_dir):
                    import shutil
                    shutil.rmtree(temp_dir)
                    self.logger.debug(f"Cleaned up temporary directory: {temp_dir}")
            
            if not upload_result.success:
                await self._update_status(upload_id, {
                    "status": UploadStatus.FAILED,
                    "status_message": f"GitHub clone failed: {upload_result.status}",
                    "error_details": upload_result.validation_errors
                })
                return AgentUpdateResult(
                    success=False,
                    agent_id=agent_id,
                    new_version=new_version,
                    previous_version=current_version,
                    status="failed",
                    error_message=upload_result.status,
                    upload_id=upload_id
                )
            
            # Update registry with new version info
            await self._update_registry_version(agent_id, new_version, current_version, description)
            
            # Update status: Triggering orchestration
            await self._update_status(upload_id, {
                "status": UploadStatus.ORCHESTRATION_TRIGGERED,
                "progress_percentage": 80,
                "status_message": "Triggering build and deployment"
            })
            
            # Trigger orchestration with GitHub update action
            orchestration_triggered = await self.orchestration_service.trigger_agent_orchestration(
                agent_name=agent_name,
                agent_path=f"/app/agents/{agent_name}/v{new_version}",
                base_url="http://nasiko-backend.nasiko.svc.cluster.local:8000",
                additional_data={
                    "action": "update_agent",
                    "agent_id": agent_id,
                    "new_version": new_version,
                    "previous_version": current_version,
                    "update_strategy": update_strategy,
                    "cleanup_old": str(cleanup_old),
                    "owner_id": user_id,
                    "upload_id": upload_id,
                    "upload_type": "github_update",
                    "repository_full_name": repository_full_name,
                    "branch": branch,
                    "description": description or ""
                }
            )
            
            if orchestration_triggered:
                await self._update_status(upload_id, {
                    "status": UploadStatus.ORCHESTRATION_PROCESSING,
                    "progress_percentage": 90,
                    "status_message": f"Building and deploying GitHub version {new_version}"
                })
                
                return AgentUpdateResult(
                    success=True,
                    agent_id=agent_id,
                    new_version=new_version,
                    previous_version=current_version,
                    update_strategy=update_strategy,
                    status="building",
                    upload_id=upload_id
                )
            else:
                await self._update_status(upload_id, {
                    "status": UploadStatus.FAILED,
                    "status_message": "Failed to trigger orchestration"
                })
                return AgentUpdateResult(
                    success=False,
                    agent_id=agent_id,
                    new_version=new_version,
                    previous_version=current_version,
                    status="failed",
                    error_message="Failed to trigger orchestration",
                    upload_id=upload_id
                )
                
        except Exception as e:
            self.logger.error(f"AGENT_UPDATE: GitHub agent update failed for {agent_id}: {str(e)}")
            await self._update_status(upload_id, {
                "status": UploadStatus.FAILED,
                "status_message": f"GitHub update failed: {str(e)}"
            })
            return AgentUpdateResult(
                success=False,
                agent_id=agent_id,
                new_version=new_version,
                previous_version=current_version,
                status="failed",
                error_message=str(e),
                upload_id=upload_id
            )

    async def _validate_agentcard_version(self, versioned_path: str, expected_version: str, agent_name: str) -> Dict[str, Any]:
        """Validate AgentCard version against expected version"""
        try:
            agentcard = await self.agentcard_service.load_agentcard_from_file(versioned_path)
            
            if not agentcard:
                return {
                    "valid": False,
                    "message": f"Version validation skipped: AgentCard.json not found in {versioned_path}",
                    "agentcard_version": None,
                    "expected_version": expected_version
                }
            
            agentcard_version = agentcard.get("version")
            if not agentcard_version:
                return {
                    "valid": False,
                    "message": f"Version validation warning: No version field found in AgentCard.json for agent {agent_name}. Expected: {expected_version}",
                    "agentcard_version": None,
                    "expected_version": expected_version
                }
            
            # Normalize versions for comparison (remove 'v' prefix)
            normalized_agentcard = agentcard_version.lstrip('v')
            normalized_expected = expected_version.lstrip('v')
            
            if normalized_agentcard == normalized_expected:
                return {
                    "valid": True,
                    "message": f"Version validation successful: AgentCard version {agentcard_version} matches expected {expected_version}",
                    "agentcard_version": agentcard_version,
                    "expected_version": expected_version
                }
            else:
                return {
                    "valid": False,
                    "message": f"Version mismatch for agent {agent_name}: AgentCard has {agentcard_version}, expected {expected_version}. Update proceeding but versions are inconsistent.",
                    "agentcard_version": agentcard_version,
                    "expected_version": expected_version
                }
                
        except Exception as e:
            return {
                "valid": False,
                "message": f"Version validation error for agent {agent_name}: {str(e)}",
                "agentcard_version": None,
                "expected_version": expected_version,
                "error": str(e)
            }