"""
Agent Operations Repository - Agent build and deployment operations
"""
from bson import ObjectId
from datetime import datetime, timezone
from .base_repository import BaseRepository


class AgentOperationsRepository(BaseRepository):
    """Repository for agent build and deployment operations"""
    
    def __init__(self, db, logger):
        super().__init__(db, logger)
        self.BuildCollection = db["agent_builds"]
        self.DeploymentCollection = db["agent_deployments"]
    
    async def ensure_indexes(self):
        """Ensure agent operations collection indexes"""
        try:
            # Build collection indexes
            await self.BuildCollection.create_index("agent_id")
            await self.BuildCollection.create_index("github_url")
            await self.BuildCollection.create_index("status")
            await self.BuildCollection.create_index("created_at")
            await self.BuildCollection.create_index("updated_at")
            
            # Deployment collection indexes
            await self.DeploymentCollection.create_index("agent_id")
            await self.DeploymentCollection.create_index("build_id")
            await self.DeploymentCollection.create_index("status")
            await self.DeploymentCollection.create_index("namespace")
            await self.DeploymentCollection.create_index("created_at")
            
            self.logger.info("Agent operations collection indexes initialized successfully")
        except Exception as e:
            self.logger.warning(f"Error ensuring agent operations indexes: {e}")
    
    # Build Operations
    async def create_agent_build(self, build_data: dict):
        """Create a new agent build record"""
        result = await self.BuildCollection.insert_one(build_data)
        return await self.BuildCollection.find_one({"_id": result.inserted_id})

    async def get_agent_build_by_id(self, build_id: ObjectId):
        """Get an agent build by ID"""
        return await self.BuildCollection.find_one({"_id": build_id})

    async def update_agent_build(self, build_id: ObjectId, update_data: dict):
        """Update an agent build record"""
        update_data["updated_at"] = datetime.now(timezone.utc)
        await self.BuildCollection.update_one(
            {"_id": build_id},
            {"$set": update_data}
        )
        return await self.get_agent_build_by_id(build_id)

    async def get_agent_builds_by_agent_id(self, agent_id: str, limit: int = 10):
        """Get builds for a specific agent"""
        cursor = self.BuildCollection.find({"agent_id": agent_id}).sort("created_at", -1).limit(limit)
        return await cursor.to_list(length=limit)

    async def get_agent_builds_by_status(self, status: str, limit: int = 100):
        """Get builds by status"""
        cursor = self.BuildCollection.find({"status": status}).sort("created_at", -1).limit(limit)
        return await cursor.to_list(length=limit)

    # Deployment Operations
    async def create_agent_deployment(self, deploy_data: dict):
        """Create a new agent deployment record"""
        result = await self.DeploymentCollection.insert_one(deploy_data)
        return await self.DeploymentCollection.find_one({"_id": result.inserted_id})

    async def get_agent_deployment_by_id(self, deploy_id: ObjectId):
        """Get an agent deployment by ID"""
        return await self.DeploymentCollection.find_one({"_id": deploy_id})

    async def update_agent_deployment(self, deploy_id: ObjectId, update_data: dict):
        """Update an agent deployment record"""
        update_data["updated_at"] = datetime.now(timezone.utc)
        await self.DeploymentCollection.update_one(
            {"_id": deploy_id},
            {"$set": update_data}
        )
        return await self.get_agent_deployment_by_id(deploy_id)

    async def get_agent_deployments_by_agent_id(self, agent_id: str, limit: int = 10):
        """Get deployments for a specific agent"""
        cursor = self.DeploymentCollection.find({"agent_id": agent_id}).sort("created_at", -1).limit(limit)
        return await cursor.to_list(length=limit)

    async def get_agent_deployment_by_build_id(self, build_id: str):
        """Get deployment by build ID"""
        return await self.DeploymentCollection.find_one({"build_id": build_id})

    async def get_active_deployments(self, namespace: str = "nasiko-agents"):
        """Get all active deployments in a namespace"""
        return await self.DeploymentCollection.find({
            "namespace": namespace,
            "status": {"$in": ["starting", "running"]}
        }).to_list(length=None)

    # Legacy method aliases for backward compatibility
    async def create_build(self, build_data: dict):
        """Legacy method - use create_agent_build instead"""
        return await self.create_agent_build(build_data)

    async def create_deployment(self, deploy_data: dict):
        """Legacy method - returns ID as string for compatibility"""
        result = await self.DeploymentCollection.insert_one(deploy_data)
        return str(result.inserted_id)

    async def update_build_status(self, build_id: str, status: str, logs: str = ''):
        """Legacy method for updating build status"""
        update = {"status": status, "updated_at": datetime.now(timezone.utc)}
        if logs:
            update["logs"] = logs
        await self.BuildCollection.update_one({"_id": ObjectId(build_id)}, {"$set": update})
    
    async def delete_agent_builds_by_agent_id(self, agent_id: str) -> int:
        """Delete all build records for an agent"""
        try:
            result = await self.BuildCollection.delete_many({"agent_id": agent_id})
            count = result.deleted_count
            self.logger.info(f"REPO: Deleted {count} build records for agent {agent_id}")
            return count
        except Exception as e:
            self.logger.error(f"REPO: Error deleting build records for agent {agent_id}: {e}")
            return 0
    
    async def delete_agent_deployments_by_agent_id(self, agent_id: str) -> int:
        """Delete all deployment records for an agent"""
        try:
            result = await self.DeploymentCollection.delete_many({"agent_id": agent_id})
            count = result.deleted_count
            self.logger.info(f"REPO: Deleted {count} deployment records for agent {agent_id}")
            return count
        except Exception as e:
            self.logger.error(f"REPO: Error deleting deployment records for agent {agent_id}: {e}")
            return 0