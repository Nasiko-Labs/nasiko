"""
Main Repository - Combines all repository modules
"""
from bson import ObjectId

from .registry_repository import RegistryRepository
from .upload_status_repository import UploadStatusRepository
from .chat_repository import ChatRepository
from .n8n_repository import N8NRepository
from .github_repository import GitHubRepository
from .agent_operations_repository import AgentOperationsRepository


class Repository:
    """Main repository class that combines all repository modules"""
    
    def __init__(self, db, logger):
        self.db = db
        self.logger = logger
        
        # Initialize all repository modules
        self.registry = RegistryRepository(db, logger)
        self.upload_status = UploadStatusRepository(db, logger)
        self.chat = ChatRepository(db, logger)
        self.n8n = N8NRepository(db, logger)
        self.github = GitHubRepository(db, logger)
        self.agent_operations = AgentOperationsRepository(db, logger)

    
    async def ensure_collections(self):
        """Ensure all collections exist and have proper indexes"""
        try:
            # Ensure indexes for all repository modules
            await self.registry.ensure_indexes()
            await self.upload_status.ensure_indexes()
            await self.chat.ensure_indexes()
            await self.n8n.ensure_indexes()
            await self.github.ensure_indexes()
            await self.agent_operations.ensure_indexes()
            
            self.logger.info("Database collections and indexes initialized successfully")
        except Exception as e:
            self.logger.warning(f"Error ensuring collections: {e}")
            # Don't fail startup if index creation fails
    
    # Registry operations (delegate to registry repository)
    async def create_registry(self, registry_data: dict):
        return await self.registry.create_registry(registry_data)
    
    async def get_all_registries(self):
        return await self.registry.get_all_registries()
    
    async def get_registry_by_id(self, registry_id):
        return await self.registry.get_registry_by_id(registry_id)
    
    async def get_registry_by_name(self, agent_name: str):
        return await self.registry.get_registry_by_name(agent_name)
    
    async def get_registry_by_agent_id(self, agent_id: str):
        return await self.registry.get_registry_by_agent_id(agent_id)

    async def update_registry(self, registry_id: ObjectId, update_data: dict):
        return await self.registry.update_registry(registry_id, update_data)
    
    # Upload status operations (delegate to upload status repository)
    async def get_upload_status_by_id(self, status_id):
        return await self.upload_status.get_upload_status_by_id(status_id)
    
    async def create_upload_status(self, upload_status_data: dict):
        return await self.upload_status.create_upload_status(upload_status_data)
    
    async def get_upload_status_by_upload_id(self, upload_id: str):
        return await self.upload_status.get_upload_status_by_upload_id(upload_id)
    
    async def update_upload_status(self, upload_id: str, update_data: dict):
        return await self.upload_status.update_upload_status(upload_id, update_data)
    
    async def update_upload_status_by_agent_name(self, agent_name: str, update_data):
        return await self.upload_status.update_upload_status_by_agent_name(agent_name, update_data)
    
    async def get_upload_status_by_agent_name(self, agent_name: str):
        return await self.upload_status.get_upload_status_by_agent_name(agent_name)
    
    async def get_upload_statuses_by_user(self, user_id: str, limit: int = 100):
        return await self.upload_status.get_upload_statuses_by_user(user_id, limit)
    
    # Chat operations (delegate to chat repository)
    async def create_session(self, user_id: str, session_id: str, agent_id=None, agent_url=None):
        return await self.chat.create_session(user_id, session_id, agent_id, agent_url)
    
    async def delete_session(self, session_id: str, user_id: str):
        return await self.chat.delete_session(session_id, user_id)
    
    async def get_session_history(self, user_id: str, limit: int = 20, cursor=None, direction: str = "after"):
        return await self.chat.get_session_history(user_id, limit, cursor, direction)
    
    async def get_chat_history(self, user_id: str, session_id: str, limit: int = 20, cursor=None, direction: str = "after"):
        return await self.chat.get_chat_history(user_id, session_id, limit, cursor, direction)
    
    # N8N credentials operations (delegate to N8N repository)
    async def get_user_n8n_credential_by_user_id(self, user_id: str):
        return await self.n8n.get_user_n8n_credential_by_user_id(user_id)
    
    async def get_user_n8n_credential_decrypted(self, user_id: str):
        return await self.n8n.get_user_n8n_credential_decrypted(user_id)
    
    async def update_user_n8n_credential(self, user_id: str, update_data: dict):
        return await self.n8n.update_user_n8n_credential(user_id, update_data)
    
    async def upsert_user_n8n_credential(self, credential_data: dict):
        return await self.n8n.upsert_user_n8n_credential(credential_data)
    
    async def delete_user_n8n_credential(self, user_id: str):
        return await self.n8n.delete_user_n8n_credential(user_id)
    
    async def update_credential_test_result(self, user_id: str, status: str):
        return await self.n8n.update_credential_test_result(user_id, status)
    
    # GitHub credentials operations (delegate to GitHub repository)
    async def get_user_github_credential_by_user_id(self, user_id: str):
        return await self.github.get_user_github_credential_by_user_id(user_id)
    
    async def get_user_github_credential_decrypted(self, user_id: str):
        return await self.github.get_user_github_credential_decrypted(user_id)
    
    async def upsert_user_github_credential(self, credential_data: dict):
        return await self.github.upsert_user_github_credential(credential_data)
    
    async def delete_user_github_credential(self, user_id: str):
        return await self.github.delete_user_github_credential(user_id)
    
    async def update_github_credential_test_result(self, user_id: str, status: str, github_user_info: dict = None):
        return await self.github.update_github_credential_test_result(user_id, status, github_user_info)
    
    # Agent operations (delegate to agent operations repository)
    async def create_agent_build(self, build_data: dict):
        return await self.agent_operations.create_agent_build(build_data)
    
    async def get_agent_build_by_id(self, build_id):
        return await self.agent_operations.get_agent_build_by_id(build_id)
    
    async def update_agent_build(self, build_id, update_data: dict):
        return await self.agent_operations.update_agent_build(build_id, update_data)
    
    async def create_agent_deployment(self, deploy_data: dict):
        return await self.agent_operations.create_agent_deployment(deploy_data)
    
    async def get_agent_deployment_by_id(self, deploy_id):
        return await self.agent_operations.get_agent_deployment_by_id(deploy_id)
    
    async def update_agent_deployment(self, deploy_id, update_data: dict):
        return await self.agent_operations.update_agent_deployment(deploy_id, update_data)
    
    async def get_agent_builds_by_agent_id(self, agent_id: str, limit: int = 10):
        return await self.agent_operations.get_agent_builds_by_agent_id(agent_id, limit)
    
    async def get_agent_deployments_by_agent_id(self, agent_id: str, limit: int = 10):
        return await self.agent_operations.get_agent_deployments_by_agent_id(agent_id, limit)
    
    # Legacy method aliases for backward compatibility
    async def create_build(self, build_data: dict):
        return await self.agent_operations.create_build(build_data)
    
    async def create_deployment(self, deploy_data: dict):
        return await self.agent_operations.create_deployment(deploy_data)
    
    async def update_build_status(self, build_id: str, status: str, logs: str = ''):
        return await self.agent_operations.update_build_status(build_id, status, logs)
    
    # Agent deletion methods
    async def delete_registry_by_agent_id(self, agent_id: str) -> bool:
        return await self.registry.delete_registry_by_agent_id(agent_id)
    
    async def delete_agent_builds_by_agent_id(self, agent_id: str) -> int:
        return await self.agent_operations.delete_agent_builds_by_agent_id(agent_id)
    
    async def delete_agent_deployments_by_agent_id(self, agent_id: str) -> int:
        return await self.agent_operations.delete_agent_deployments_by_agent_id(agent_id)
    
    async def delete_upload_status_by_agent_id(self, agent_id: str) -> int:
        return await self.upload_status.delete_upload_status_by_agent_id(agent_id)