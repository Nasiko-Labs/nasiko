"""
Upload Status Repository - Upload tracking operations
"""
from datetime import datetime, timezone
from bson import ObjectId
from .base_repository import BaseRepository


class UploadStatusRepository(BaseRepository):
    """Repository for upload status tracking operations"""
    
    def __init__(self, db, logger):
        super().__init__(db, logger)
        self.UploadStatusCollection = db["upload-status"]
    
    async def ensure_indexes(self):
        """Ensure upload status collection indexes"""
        try:
            await self.UploadStatusCollection.create_index("upload_id", unique=True)
            await self.UploadStatusCollection.create_index("agent_name")
            await self.UploadStatusCollection.create_index("status")
            await self.UploadStatusCollection.create_index("owner_id")
            await self.UploadStatusCollection.create_index("created_at")
            await self.UploadStatusCollection.create_index(
                [("agent_name", 1), ("created_at", -1)]
            )
            await self.UploadStatusCollection.create_index(
                [("owner_id", 1), ("created_at", -1)]
            )
            
            self.logger.info("Upload status collection indexes initialized successfully")
        except Exception as e:
            self.logger.warning(f"Error ensuring upload status indexes: {e}")
    
    async def get_upload_status_by_id(self, status_id: ObjectId):
        """Get upload status by database ID"""
        return await self.UploadStatusCollection.find_one({"_id": status_id})
    
    async def create_upload_status(self, upload_status_data: dict):
        """Create a new upload status entry"""
        result = await self.UploadStatusCollection.insert_one(upload_status_data)
        return await self.get_upload_status_by_id(result.inserted_id)
    
    async def get_upload_status_by_upload_id(self, upload_id: str):
        """Get upload status by upload ID"""
        return await self.UploadStatusCollection.find_one({"upload_id": upload_id})
    
    async def update_upload_status(self, upload_id: str, update_data: dict):
        """Update upload status by upload ID"""
        update_data["updated_at"] = datetime.now(timezone.utc)
        
        await self.UploadStatusCollection.update_one(
            {"upload_id": upload_id}, {"$set": update_data}
        )
        return await self.get_upload_status_by_upload_id(upload_id)
    
    async def update_upload_status_by_agent_name(self, agent_name: str, update_data):
        """Update the most recent upload status for an agent"""
        # Convert Pydantic model to dict if needed
        if hasattr(update_data, "model_dump"):
            # Pydantic v2 model
            update_dict = update_data.model_dump(exclude_none=True)
        elif hasattr(update_data, "dict"):
            # Pydantic v1 model
            update_dict = update_data.dict(exclude_none=True)
        else:
            # Already a dict
            update_dict = (
                dict(update_data) if not isinstance(update_data, dict) else update_data
            )
        
        # Add updated timestamp
        update_dict["updated_at"] = datetime.now(timezone.utc)
        
        # Find the most recent upload for this agent
        latest_upload = await self.UploadStatusCollection.find_one(
            {"agent_name": agent_name}, sort=[("created_at", -1)]
        )
        
        if latest_upload:
            await self.UploadStatusCollection.update_one(
                {"upload_id": latest_upload["upload_id"]}, {"$set": update_dict}
            )
            return await self.get_upload_status_by_upload_id(latest_upload["upload_id"])
        return None
    
    async def get_upload_status_by_agent_name(self, agent_name: str):
        """Get upload statuses by agent name, sorted by most recent first"""
        cursor = self.UploadStatusCollection.find({"agent_name": agent_name}).sort("created_at", -1)
        return await cursor.to_list(None)

    async def get_upload_statuses_by_user(self, user_id: str, limit: int = 100):
        """Get all upload statuses for a specific user"""
        cursor = (
            self.UploadStatusCollection.find({"owner_id": user_id})
            .sort("created_at", -1)
            .limit(limit)
        )
        return await cursor.to_list(length=None)
    
    async def delete_upload_status_by_agent_id(self, agent_id: str) -> int:
        """Delete all upload status records for an agent"""
        try:
            result = await self.UploadStatusCollection.delete_many({"agent_name": agent_id})
            count = result.deleted_count
            self.logger.info(f"REPO: Deleted {count} upload status records for agent {agent_id}")
            return count
        except Exception as e:
            self.logger.error(f"REPO: Error deleting upload status records for agent {agent_id}: {e}")
            return 0