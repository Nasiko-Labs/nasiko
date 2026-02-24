"""
Registry Repository - Agent registry operations
"""
from bson import ObjectId
from .base_repository import BaseRepository


class RegistryRepository(BaseRepository):
    """Repository for agent registry operations"""
    
    def __init__(self, db, logger):
        super().__init__(db, logger)
        self.RegistryCollection = db["registry"]
    
    async def ensure_indexes(self):
        """Ensure registry collection indexes"""
        try:
            # Core registry indexes
            await self.RegistryCollection.create_index("name", unique=True)
            await self.RegistryCollection.create_index("id", unique=True)
            await self.RegistryCollection.create_index("created_at")
            await self.RegistryCollection.create_index("updated_at")
            
            # Version management indexes
            await self.RegistryCollection.create_index("version")
            await self.RegistryCollection.create_index([("id", 1), ("version", 1)])
            await self.RegistryCollection.create_index("version_history.status")
            
            self.logger.info("Registry collection indexes initialized successfully")
        except Exception as e:
            self.logger.warning(f"Error ensuring registry indexes: {e}")
    
    async def create_registry(self, registry_data: dict):
        """Create a new registry entry with version support"""
        from datetime import datetime, timezone
        current_time = datetime.now(timezone.utc)
        
        # Ensure version fields are present for new registries
        if "version" not in registry_data:
            # Default to 1.0.0 only if no version provided in AgentCard
            registry_data["version"] = "1.0.0"
        
        # Normalize version format (remove 'v' prefix for database storage)
        version = registry_data["version"]
        if version.startswith('v'):
            registry_data["version"] = version[1:]
        
        if "version_history" not in registry_data:
            initial_version_info = {
                "version": registry_data["version"],
                "status": "active",
                "created_at": current_time.isoformat(),
                "build_ids": [],
                "deployment_ids": [],
                "description": "Initial version",
                "rollback_info": {
                    "can_rollback": False,
                    "previous_version": None
                }
            }
            registry_data["version_history"] = [initial_version_info]
        
        # Ensure timestamps
        if "created_at" not in registry_data:
            registry_data["created_at"] = current_time
        if "updated_at" not in registry_data:
            registry_data["updated_at"] = current_time
        
        result = await self.RegistryCollection.insert_one(registry_data)
        return await self.get_registry_by_id(result.inserted_id)
    
    async def get_all_registries(self):
        """Get all registry entries"""
        cursor = self.RegistryCollection.find()
        return await cursor.to_list(length=None)
    
    async def get_registry_by_id(self, registry_id: ObjectId):
        """Get registry by database ID"""
        return await self.RegistryCollection.find_one({"_id": registry_id})
    
    async def get_registry_by_name(self, agent_name: str):
        """Get registry by agent name"""
        self.logger.info(f"REPO: Looking for registry with name: {agent_name}")
        result = await self.RegistryCollection.find_one({"name": agent_name})
        self.logger.info(f"REPO: Found existing registry: {result is not None}")
        return result
    
    async def get_registry_by_agent_id(self, agent_id: str):
        """Get registry by agent ID with version field normalization"""
        self.logger.info(f"REPO: Looking for registry with id: {agent_id}")
        result = await self.RegistryCollection.find_one({"id": agent_id})
        self.logger.info(f"REPO: Found existing registry: {result is not None}")
        
        # Add version fields to existing entries for backward compatibility
        if result and "version" not in result:
            result = self._normalize_version_fields(result)
        
        return result

    async def update_registry(self, registry_id: ObjectId, update_data: dict):
        await self.RegistryCollection.update_one(
            {"_id": registry_id}, {"$set": update_data}
        )
        return await self.get_registry_by_id(registry_id)
    
    async def delete_registry_by_agent_id(self, agent_id: str) -> bool:
        """Delete registry entry by agent ID"""
        try:
            result = await self.RegistryCollection.delete_one({"id": agent_id})
            deleted = result.deleted_count > 0
            self.logger.info(f"REPO: Registry deletion for agent {agent_id}: {'success' if deleted else 'not found'}")
            return deleted
        except Exception as e:
            self.logger.error(f"REPO: Error deleting registry for agent {agent_id}: {e}")
            return False

    def _normalize_version_fields(self, registry_entry: dict) -> dict:
        """Add default version fields to existing registry entries"""
        from datetime import datetime, timezone
        
        # Don't modify the original dict
        result = registry_entry.copy()
        
        # Add version if missing
        if "version" not in result:
            result["version"] = "1.0.0"
        
        # Add version_history if missing
        if "version_history" not in result:
            current_time = registry_entry.get("created_at")
            if not current_time:
                current_time = datetime.now(timezone.utc).isoformat()
            elif hasattr(current_time, 'isoformat'):
                current_time = current_time.isoformat()
            
            initial_version_info = {
                "version": result["version"],
                "status": "active",
                "created_at": current_time,
                "build_ids": [],
                "deployment_ids": [],
                "description": "Initial version (auto-generated)",
                "rollback_info": {
                    "can_rollback": False,
                    "previous_version": None
                }
            }
            result["version_history"] = [initial_version_info]
        
        return result