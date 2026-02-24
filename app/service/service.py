from app.repository.repository import Repository
from app.entity.entity import RegistryBase, RegistryInDB
from datetime import datetime, timezone
from typing import List, Optional, Dict
from app.service.k8s_service import K8sService
import httpx
from app.pkg.config.config import settings
from app.api.types import GithubUser, Token, GithubLoginResponse
from app.pkg.redisclient.redisclient import get_github_access_token, set_github_access_token, clear_github_access_token

def convert_objectid_to_str(doc: dict) -> dict:
    """Convert ObjectId fields to strings for Pydantic compatibility"""
    if doc and "_id" in doc:
        doc["_id"] = str(doc["_id"])
    return doc

def extract_and_deduplicate_tags_from_skills(skills: List[Dict]) -> List[str]:
    """Extract and deduplicate tags from skills array"""
    all_tags = []
    
    for skill in skills:
        if isinstance(skill, dict) and "tags" in skill:
            tags = skill["tags"]
            if isinstance(tags, list):
                all_tags.extend(tags)
    
    # Remove duplicates while preserving order
    seen = set()
    deduplicated_tags = []
    for tag in all_tags:
        if isinstance(tag, str) and tag not in seen:
            seen.add(tag)
            deduplicated_tags.append(tag)
    
    return deduplicated_tags

class Service:
    def __init__(self, repo: Repository, logger):
        self.repo = repo
        self.logger = logger
        self.k8s_service = K8sService(logger) # Initialize K8s Service

    ## Agent Registry Service Methods
    
    async def create_registry(self, registry_data: RegistryBase) -> Optional[RegistryInDB]:
        registry_dict = registry_data.model_dump()
        registry_dict["created_at"] = datetime.now(timezone.utc)
        registry_dict["updated_at"] = datetime.now(timezone.utc)

        # Extract and deduplicate tags from skills
        if registry_data.skills:
            skills_dicts = [skill.dict() if hasattr(skill, 'dict') else skill for skill in registry_data.skills]
            registry_dict["tags"] = extract_and_deduplicate_tags_from_skills(skills_dicts)

        # Check if agent with same name already exists (using new schema)
        agent_name = registry_data.name
        existing = await self.repo.get_registry_by_name(agent_name)
        if existing:
            raise ValueError(f"Agent registry with name '{agent_name}' already exists")

        result = await self.repo.create_registry(registry_dict)
        if result:
            result["_id"] = str(result["_id"])  # Convert ObjectId to string
            return RegistryInDB(**result)
        return None

    async def get_all_registries(self) -> List[RegistryInDB]:
        registries = await self.repo.get_all_registries()
        return [RegistryInDB(**convert_objectid_to_str(registry)) for registry in registries]

    async def get_registry_by_name(self, agent_name: str) -> Optional[RegistryInDB]:
        result = await self.repo.get_registry_by_name(agent_name)
        if result:
            return RegistryInDB(**convert_objectid_to_str(result))
        return None

    async def get_registry_by_agent_id(self, agent_id: str) -> Optional[RegistryInDB]:
        result = await self.repo.get_registry_by_agent_id(agent_id)
        if result:
            return RegistryInDB(**convert_objectid_to_str(result))
        return None

    def get_github_access_token(self) -> Optional[str]:
        return get_github_access_token()

    ## Upload Status Service Methods

    async def get_upload_statuses_by_user(self, user_id: str, limit: int = 100) -> List[Dict]:
        """Get all upload statuses for a specific user"""
        try:
            self.logger.info(f"SERVICE: Getting upload statuses for user: {user_id}")
            upload_statuses = await self.repo.get_upload_statuses_by_user(user_id, limit)
            self.logger.info(f"SERVICE: Found {len(upload_statuses)} upload statuses for user {user_id}")
            return upload_statuses
        except Exception as e:
            self.logger.error(f"SERVICE: Error getting upload statuses for user {user_id}: {str(e)}")
            raise e

    async def upsert_registry_by_name(self, registry_name: str, upsert_data: RegistryBase) -> Optional[RegistryInDB]:
        try:
            self.logger.info(f"SERVICE: Upserting registry with name: {registry_name}")

            # DEPRECATED: Old approach using nested agent.id
            # # Check if registry with this agent.id already exists
            # agent_id = upsert_data.agent.id if hasattr(upsert_data, 'agent') and hasattr(upsert_data.agent, 'id') else None

            # New approach: Use top-level id field from AgentCard format
            agent_id = upsert_data.id if hasattr(upsert_data, 'id') else None

            if agent_id:
                self.logger.info(f"SERVICE: Looking for existing registry with id: {agent_id}")
                existing_registry = await self.repo.get_registry_by_agent_id(agent_id)
            else:
                self.logger.warning("SERVICE: No agent.id found in upsert_data, falling back to name lookup")
                existing_registry = await self.repo.get_registry_by_name(registry_name)

            if existing_registry:
                self.logger.info("SERVICE: Found existing registry, updating...")
                # Update existing registry - use the existing _id field
                update_dict = upsert_data.model_dump()
                update_dict["updated_at"] = datetime.now(timezone.utc)

                # Extract and deduplicate tags from skills
                if upsert_data.skills:
                    skills_dicts = [skill.dict() if hasattr(skill, 'dict') else skill for skill in upsert_data.skills]
                    update_dict["tags"] = extract_and_deduplicate_tags_from_skills(skills_dicts)

                # existing_registry["_id"] is already an ObjectId from MongoDB
                result = await self.repo.update_registry(existing_registry["_id"], update_dict)
                if result:
                    self.logger.info("SERVICE: Successfully updated registry")
                    return RegistryInDB(**convert_objectid_to_str(result))
            else:
                self.logger.info("SERVICE: No existing registry found, creating new one...")
                # Create new registry
                registry_dict = upsert_data.model_dump()
                registry_dict["created_at"] = datetime.now(timezone.utc)
                registry_dict["updated_at"] = datetime.now(timezone.utc)

                # Extract and deduplicate tags from skills
                if upsert_data.skills:
                    skills_dicts = [skill.dict() if hasattr(skill, 'dict') else skill for skill in upsert_data.skills]
                    registry_dict["tags"] = extract_and_deduplicate_tags_from_skills(skills_dicts)

                result = await self.repo.create_registry(registry_dict)
                if result:
                    result["_id"] = str(result["_id"])
                    self.logger.info("SERVICE: Successfully created new registry")
                    return RegistryInDB(**result)

            self.logger.error(f"SERVICE: Failed to upsert registry - no result returned")
            return None
        except Exception as e:
            self.logger.error(f"SERVICE: Exception in upsert_registry_by_name: {type(e).__name__}: {str(e)}")
            import traceback
            self.logger.error(f"SERVICE: Traceback: {traceback.format_exc()}")
            raise
    
    async def delete_agent_completely(self, agent_id: str, user_id: str) -> Dict:
        """Delete an agent and all related resources (K8s deployments, permissions, registry, database records)"""
        try:
            self.logger.info(f"SERVICE: Starting complete deletion for agent {agent_id}")
            
            deletion_results = {
                "registry_deleted": False,
                "k8s_deployments_deleted": [],
                "permissions_deleted": False,
                "build_records_deleted": 0,
                "deployment_records_deleted": 0,
                "upload_records_deleted": 0,
                "errors": []
            }
            
            # Step 1: Get registry info before deletion to find K8s deployments
            try:
                registry = await self.repo.get_registry_by_agent_id(agent_id)
                if registry:
                    self.logger.info(f"SERVICE: Found registry entry for agent {agent_id}")
                else:
                    self.logger.warning(f"SERVICE: No registry entry found for agent {agent_id}")
            except Exception as e:
                self.logger.warning(f"SERVICE: Error getting registry for {agent_id}: {e}")
                registry = None
            
            # Step 2: Delete K8s deployments and services
            try:
                # Find all deployments that start with agent name
                k8s_deletions = await self._delete_agent_k8s_resources(agent_id)
                deletion_results["k8s_deployments_deleted"] = k8s_deletions
                self.logger.info(f"SERVICE: Deleted {len(k8s_deletions)} K8s resources for agent {agent_id}")
            except Exception as e:
                error_msg = f"K8s deletion failed: {str(e)}"
                deletion_results["errors"].append(error_msg)
                self.logger.error(f"SERVICE: {error_msg}")
            
            # Step 3: Delete agent permissions
            try:
                permissions_deleted = await self._delete_agent_permissions(agent_id)
                deletion_results["permissions_deleted"] = permissions_deleted
                if permissions_deleted:
                    self.logger.info(f"SERVICE: Deleted permissions for agent {agent_id}")
                else:
                    self.logger.warning(f"SERVICE: No permissions found or failed to delete for agent {agent_id}")
            except Exception as e:
                error_msg = f"Permissions deletion failed: {str(e)}"
                deletion_results["errors"].append(error_msg)
                self.logger.error(f"SERVICE: {error_msg}")
            
            # Step 4: Delete database records
            try:
                # Delete build records
                build_count = await self._delete_agent_build_records(agent_id)
                deletion_results["build_records_deleted"] = build_count
                
                # Delete deployment records  
                deployment_count = await self._delete_agent_deployment_records(agent_id)
                deletion_results["deployment_records_deleted"] = deployment_count
                
                # Delete upload records
                upload_count = await self._delete_agent_upload_records(agent_id)
                deletion_results["upload_records_deleted"] = upload_count
                
                self.logger.info(f"SERVICE: Deleted {build_count} builds, {deployment_count} deployments, {upload_count} uploads")
            except Exception as e:
                error_msg = f"Database cleanup failed: {str(e)}"
                deletion_results["errors"].append(error_msg)
                self.logger.error(f"SERVICE: {error_msg}")
            
            # Step 5: Delete registry entry (do this last in case other steps need the registry info)
            try:
                if registry:
                    registry_deleted = await self.repo.delete_registry_by_agent_id(agent_id)
                    deletion_results["registry_deleted"] = registry_deleted
                    if registry_deleted:
                        self.logger.info(f"SERVICE: Deleted registry entry for agent {agent_id}")
                    else:
                        self.logger.warning(f"SERVICE: Failed to delete registry entry for agent {agent_id}")
                else:
                    self.logger.info(f"SERVICE: No registry entry to delete for agent {agent_id}")
            except Exception as e:
                error_msg = f"Registry deletion failed: {str(e)}"
                deletion_results["errors"].append(error_msg)
                self.logger.error(f"SERVICE: {error_msg}")
            
            # Determine overall success
            has_critical_errors = any("K8s deletion failed" in error or "Database cleanup failed" in error for error in deletion_results["errors"])
            success = not has_critical_errors
            
            self.logger.info(f"SERVICE: Agent {agent_id} deletion completed. Success: {success}")
            
            return {
                "success": success,
                "details": deletion_results
            }
            
        except Exception as e:
            self.logger.error(f"SERVICE: Critical error in delete_agent_completely for {agent_id}: {str(e)}")
            import traceback
            self.logger.error(f"SERVICE: Traceback: {traceback.format_exc()}")
            return {
                "success": False,
                "error": str(e),
                "details": deletion_results if 'deletion_results' in locals() else {}
            }
    
    async def _delete_agent_k8s_resources(self, agent_id: str) -> list:
        """Delete all K8s deployments and services for an agent"""
        try:
            deleted_resources = []
            
            # Get all deployments that match the agent pattern
            # K8s deployments are typically named like: agent-{agent_name}-{timestamp}
            deployment_names = self.k8s_service.list_agent_deployments(agent_id)
            
            for deployment_name in deployment_names:
                try:
                    # Delete deployment and associated service
                    if self.k8s_service.delete_agent_deployment(deployment_name):
                        deleted_resources.append(deployment_name)
                        self.logger.info(f"SERVICE: Deleted K8s deployment: {deployment_name}")
                    else:
                        self.logger.warning(f"SERVICE: Failed to delete K8s deployment: {deployment_name}")
                except Exception as e:
                    self.logger.error(f"SERVICE: Error deleting K8s deployment {deployment_name}: {e}")
            
            return deleted_resources
        except Exception as e:
            self.logger.error(f"SERVICE: Error in _delete_agent_k8s_resources: {e}")
            return []
    
    async def _delete_agent_permissions(self, agent_id: str) -> bool:
        """Delete agent permissions from auth service"""
        try:
            import os
            import aiohttp
            
            auth_service_url = os.environ.get("AUTH_SERVICE_URL")
            if not auth_service_url:
                self.logger.warning("SERVICE: AUTH_SERVICE_URL not configured, skipping permissions deletion")
                return False
            
            url = f"{auth_service_url}/auth/agents/{agent_id}/permissions"
            
            async with aiohttp.ClientSession() as session:
                async with session.delete(url, timeout=aiohttp.ClientTimeout(total=30)) as response:
                    if response.status in [200, 204, 404]:  # 404 is OK - means no permissions existed
                        return True
                    else:
                        error_detail = await response.text()
                        self.logger.error(f"SERVICE: Failed to delete permissions for {agent_id}: {response.status} - {error_detail}")
                        return False
                        
        except Exception as e:
            self.logger.error(f"SERVICE: Error deleting permissions for {agent_id}: {e}")
            return False
    
    async def _delete_agent_build_records(self, agent_id: str) -> int:
        """Delete all build records for an agent"""
        try:
            return await self.repo.delete_agent_builds_by_agent_id(agent_id)
        except Exception as e:
            self.logger.error(f"SERVICE: Error deleting build records for {agent_id}: {e}")
            return 0
    
    async def _delete_agent_deployment_records(self, agent_id: str) -> int:
        """Delete all deployment records for an agent"""
        try:
            return await self.repo.delete_agent_deployments_by_agent_id(agent_id)
        except Exception as e:
            self.logger.error(f"SERVICE: Error deleting deployment records for {agent_id}: {e}")
            return 0
    
    async def _delete_agent_upload_records(self, agent_id: str) -> int:
        """Delete all upload records for an agent"""
        try:
            return await self.repo.delete_upload_status_by_agent_id(agent_id)
        except Exception as e:
            self.logger.error(f"SERVICE: Error deleting upload records for {agent_id}: {e}")
            return 0
