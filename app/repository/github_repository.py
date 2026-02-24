"""
GitHub Repository - GitHub credentials operations
"""
from datetime import datetime, timezone
from .base_repository import BaseRepository


class GitHubRepository(BaseRepository):
    """Repository for GitHub user credentials operations"""
    
    def __init__(self, db, logger):
        super().__init__(db, logger)
        self.UserGitHubCredentialsCollection = db["user-github-credentials"]
    
    async def ensure_indexes(self):
        """Ensure GitHub credentials collection indexes"""
        try:
            await self.UserGitHubCredentialsCollection.create_index("user_id", unique=True)
            await self.UserGitHubCredentialsCollection.create_index("credential_type")
            await self.UserGitHubCredentialsCollection.create_index("is_active")
            await self.UserGitHubCredentialsCollection.create_index("connection_status")
            await self.UserGitHubCredentialsCollection.create_index("last_tested")
            await self.UserGitHubCredentialsCollection.create_index("created_at")
            
            self.logger.info("GitHub credentials collection indexes initialized successfully")
        except Exception as e:
            self.logger.warning(f"Error ensuring GitHub credentials indexes: {e}")
    
    async def get_user_github_credential_by_user_id(self, user_id: str) -> dict:
        """Get user GitHub credential by user ID"""
        return await self.UserGitHubCredentialsCollection.find_one({"user_id": user_id})
    
    async def get_user_github_credential_decrypted(self, user_id: str) -> dict:
        """Get user GitHub credential with decrypted access token for service use"""
        credential = await self.get_user_github_credential_by_user_id(user_id)
        if credential and "encrypted_access_token" in credential:
            credential["access_token"] = self._decrypt_data(credential["encrypted_access_token"])
            # Remove encrypted version for cleaner response
            del credential["encrypted_access_token"]
        return credential
    
    async def upsert_user_github_credential(self, credential_data: dict) -> dict:
        """Create or update user GitHub credential"""
        # Encrypt the access token before storing
        if "access_token" in credential_data:
            credential_data["encrypted_access_token"] = self._encrypt_data(credential_data["access_token"])
            del credential_data["access_token"]  # Remove plain text access token
        
        credential_data["updated_at"] = datetime.now(timezone.utc)
        
        # Remove created_at if present to avoid conflict with $setOnInsert
        if "created_at" in credential_data:
            del credential_data["created_at"]
        
        result = await self.UserGitHubCredentialsCollection.update_one(
            {"user_id": credential_data["user_id"]},
            {
                "$set": credential_data,
                "$setOnInsert": {"created_at": datetime.now(timezone.utc)},
            },
            upsert=True,
        )
        
        return await self.get_user_github_credential_by_user_id(credential_data["user_id"])
    
    async def delete_user_github_credential(self, user_id: str) -> bool:
        """Delete user GitHub credential"""
        result = await self.UserGitHubCredentialsCollection.delete_one({"user_id": user_id})
        return result.deleted_count > 0
    
    async def update_github_credential_test_result(
        self, user_id: str, status: str, github_user_info: dict = None
    ) -> dict:
        """Update the last test result for user GitHub credential"""
        update_data = {
            "last_tested": datetime.now(timezone.utc),
            "connection_status": status,
            "updated_at": datetime.now(timezone.utc),
        }
        
        # Update GitHub user info if provided
        if github_user_info:
            update_data.update(
                {
                    "github_username": github_user_info.get("login"),
                    "github_user_id": str(github_user_info.get("id")),
                    "avatar_url": github_user_info.get("avatar_url"),
                }
            )
        
        result = await self.UserGitHubCredentialsCollection.update_one(
            {"user_id": user_id}, {"$set": update_data}
        )
        
        if result.modified_count > 0:
            return await self.get_user_github_credential_by_user_id(user_id)
        return None