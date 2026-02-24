"""
N8N Repository - N8N credentials operations
"""
from datetime import datetime, timezone
from .base_repository import BaseRepository


class N8NRepository(BaseRepository):
    """Repository for N8N user credentials operations"""
    
    def __init__(self, db, logger):
        super().__init__(db, logger)
        self.UserN8NCredentialsCollection = db["user-n8n-credentials"]
    
    async def ensure_indexes(self):
        """Ensure N8N credentials collection indexes"""
        try:
            await self.UserN8NCredentialsCollection.create_index("user_id", unique=True)
            await self.UserN8NCredentialsCollection.create_index("credential_type")
            await self.UserN8NCredentialsCollection.create_index("is_active")
            await self.UserN8NCredentialsCollection.create_index("last_tested")
            await self.UserN8NCredentialsCollection.create_index("created_at")
            
            self.logger.info("N8N credentials collection indexes initialized successfully")
        except Exception as e:
            self.logger.warning(f"Error ensuring N8N credentials indexes: {e}")
    
    async def get_user_n8n_credential_by_user_id(self, user_id: str) -> dict:
        """Get user N8N credential by user ID"""
        return await self.UserN8NCredentialsCollection.find_one({"user_id": user_id})
    
    async def get_user_n8n_credential_decrypted(self, user_id: str) -> dict:
        """Get user N8N credential with decrypted API key for service use"""
        credential = await self.get_user_n8n_credential_by_user_id(user_id)
        if credential and "encrypted_api_key" in credential:
            credential["api_key"] = self._decrypt_data(credential["encrypted_api_key"])
            # Remove encrypted version for cleaner response
            del credential["encrypted_api_key"]
        return credential
    
    async def update_user_n8n_credential(self, user_id: str, update_data: dict) -> dict | None:
        """Update user N8N credential"""
        # Encrypt API key if provided
        if "api_key" in update_data:
            update_data["encrypted_api_key"] = self._encrypt_data(update_data["api_key"])
            del update_data["api_key"]  # Remove plain text API key
        
        update_data["updated_at"] = datetime.now(timezone.utc)
        
        result = await self.UserN8NCredentialsCollection.update_one(
            {"user_id": user_id}, {"$set": update_data}
        )
        
        if result.modified_count > 0:
            return await self.get_user_n8n_credential_by_user_id(user_id)
        return None
    
    async def upsert_user_n8n_credential(self, credential_data: dict) -> dict:
        """Create or update user N8N credential"""
        # Encrypt the API key before storing
        if "api_key" in credential_data:
            credential_data["encrypted_api_key"] = self._encrypt_data(credential_data["api_key"])
            del credential_data["api_key"]  # Remove plain text API key
        
        credential_data["updated_at"] = datetime.now(timezone.utc)
        
        # Remove created_at if present to avoid conflict with $setOnInsert
        if "created_at" in credential_data:
            del credential_data["created_at"]
        
        result = await self.UserN8NCredentialsCollection.update_one(
            {"user_id": credential_data["user_id"]},
            {
                "$set": credential_data,
                "$setOnInsert": {"created_at": datetime.now(timezone.utc)},
            },
            upsert=True,
        )
        
        return await self.get_user_n8n_credential_by_user_id(credential_data["user_id"])
    
    async def delete_user_n8n_credential(self, user_id: str) -> bool:
        """Delete user N8N credential"""
        result = await self.UserN8NCredentialsCollection.delete_one({"user_id": user_id})
        return result.deleted_count > 0
    
    async def update_credential_test_result(self, user_id: str, status: str) -> dict | None:
        """Update the last test result for user N8N credential"""
        update_data = {
            "last_tested": datetime.now(timezone.utc),
            "connection_status": status,
            "updated_at": datetime.now(timezone.utc),
        }
        
        result = await self.UserN8NCredentialsCollection.update_one(
            {"user_id": user_id}, {"$set": update_data}
        )
        
        if result.modified_count > 0:
            return await self.get_user_n8n_credential_by_user_id(user_id)
        return None