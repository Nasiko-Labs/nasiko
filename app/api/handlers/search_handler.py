"""
Search Handler - Handles search operations for users and agents
"""

from fastapi import HTTPException, status
from .base_handler import BaseHandler
from ..types import (
    UserSearchResponse, AgentSearchResponse, UserSearchResult, AgentSearchResult
)
from app.service.redis_search_service import RedisSearchService
from typing import Optional

class SearchHandler(BaseHandler):
    """Handler for search operations"""

    def __init__(self, service, logger):
        super().__init__(service, logger)
        self.redis_search_service = RedisSearchService(logger)
        
    async def initialize_search(self):
        """Initialize Redis search service"""
        try:
            success = await self.redis_search_service.initialize()
            if success:
                self.log_info("Redis search service initialized successfully")
                # Initial data sync can be implemented here
                await self._sync_initial_data()
            else:
                self.log_warning("Redis search service initialization failed - search functionality will be limited")
            return success
        except Exception as e:
            self.log_error("Redis search service initialization error", e)
            return False
    
    async def _sync_initial_data(self):
        """Sync initial data from database to Redis search"""
        try:
            self.log_info("Starting initial data sync to Redis search")
            
            # Sync users from auth service
            await self._sync_users()
            
            # Sync agents from registry
            await self._sync_agents()
            
            self.log_info("Initial data sync completed")
            
        except Exception as e:
            self.log_error("Initial data sync failed", e)
    
    async def _sync_users(self):
        """Sync all users to Redis search"""
        try:
            # Call auth service system endpoint for users
            import httpx
            import os
            
            auth_service_url = os.getenv("AUTH_SERVICE_URL", "http://nasiko-auth-service:8001")
            async with httpx.AsyncClient(timeout=30.0) as client:
                response = await client.get(f"{auth_service_url}/auth/system/users-for-search")
                
                if response.status_code == 200:
                    auth_response = response.json()
                    users_data = auth_response.get("users", [])
                    
                    if users_data:
                        formatted_users = []
                        for user in users_data:
                            formatted_users.append({
                                "id": user["id"],
                                "username": user["username"], 
                                "display_name": user.get("display_name"),
                                "email": user.get("email"),
                                "role": user.get("role", "User"),
                                "avatar_url": user.get("avatar_url"),
                                "is_active": user.get("is_active", True),
                                "created_at": user.get("created_at"),
                                "updated_at": user.get("updated_at")
                            })
                        
                        synced_count = await self.redis_search_service.bulk_index_users(formatted_users)
                        self.log_info(f"Synced {synced_count} users to Redis search")
                    else:
                        self.log_info("No users found in auth service for indexing")
                else:
                    self.log_warning(f"Failed to get users from auth service: {response.status_code} - {response.text}")
                    
        except Exception as e:
            self.log_error("User sync failed", e)
    
    async def _sync_agents(self):
        """Sync all agents to Redis search"""
        try:
            # Get all agents from registry
            registries = await self.service.get_all_registries()
            if registries:
                formatted_agents = []
                for registry in registries:
                    # Extract tags from registry if available
                    tags = []
                    if hasattr(registry, 'tags') and registry.tags:
                        tags = registry.tags
                    elif hasattr(registry, 'capabilities') and registry.capabilities:
                        # Extract tags from capabilities if available
                        capabilities = registry.capabilities
                        if hasattr(capabilities, 'model_dump'):
                            caps_dict = capabilities.model_dump()
                        elif isinstance(capabilities, dict):
                            caps_dict = capabilities
                        else:
                            caps_dict = {}
                        
                        # Look for tags in various places in capabilities
                        if 'tags' in caps_dict:
                            tags = caps_dict['tags']
                    
                    formatted_agents.append({
                        "agent_id": registry.id,
                        "name": registry.name,
                        "description": registry.description,
                        "tags": tags,
                        "icon_url": getattr(registry, 'iconUrl', None),
                        "owner_id": getattr(registry, 'owner_id', None),
                        "version": registry.version,
                        "url": registry.url,
                        "created_at": getattr(registry, 'created_at', None),
                        "updated_at": getattr(registry, 'updated_at', None)
                    })
                
                synced_count = await self.redis_search_service.bulk_index_agents(formatted_agents)
                self.log_info(f"Synced {synced_count} agents to Redis search")
            
        except Exception as e:
            self.log_error("Agent sync failed", e)

    async def search_users(self, query: str, limit: int = 10) -> UserSearchResponse:
        """Search for users with autocomplete functionality"""
        try:
            self.log_info("Searching users", query=query, limit=limit)
            
            # Validate query
            if len(query.strip()) < 2:
                return UserSearchResponse(
                    data=[],
                    query=query,
                    total_matches=0,
                    showing=0,
                    status_code=200,
                    message="Query too short - minimum 2 characters required"
                )
            
            # Search using Redis
            search_result = await self.redis_search_service.search_users(query, limit)
            
            if "error" in search_result:
                self.log_warning(f"User search error: {search_result['error']}")
                return UserSearchResponse(
                    data=[],
                    query=query,
                    total_matches=0,
                    showing=0,
                    status_code=200,
                    message=f"Search unavailable: {search_result['error']}"
                )
            
            # Convert to response format
            users = []
            for user in search_result["users"]:
                users.append(UserSearchResult(
                    id=user["id"],
                    username=user["username"],
                    display_name=user.get("display_name"),
                    email=user.get("email"),
                    role=user.get("role"),
                    avatar_url=user.get("avatar_url"),
                    score=user.get("score")
                ))
            
            total_matches = search_result["total"]
            showing = len(users)
            
            self.log_info("User search completed", 
                         query=query, total=total_matches, showing=showing)
            
            return UserSearchResponse(
                data=users,
                query=query,
                total_matches=total_matches,
                showing=showing,
                status_code=200,
                message=f"Found {showing} users matching '{query}'"
            )
            
        except Exception as e:
            await self.handle_service_error("search_users", e)

    async def search_agents(self, query: str, limit: int = 10) -> AgentSearchResponse:
        """Search for agents with autocomplete functionality"""
        try:
            self.log_info("Searching agents", query=query, limit=limit)
            
            # Validate query
            if len(query.strip()) < 2:
                return AgentSearchResponse(
                    data=[],
                    query=query,
                    total_matches=0,
                    showing=0,
                    status_code=200,
                    message="Query too short - minimum 2 characters required"
                )
            
            # Search using Redis
            search_result = await self.redis_search_service.search_agents(query, limit)
            
            if "error" in search_result:
                self.log_warning(f"Agent search error: {search_result['error']}")
                return AgentSearchResponse(
                    data=[],
                    query=query,
                    total_matches=0,
                    showing=0,
                    status_code=200,
                    message=f"Search unavailable: {search_result['error']}"
                )
            
            # Convert to response format
            agents = []
            for agent in search_result["agents"]:
                agents.append(AgentSearchResult(
                    agent_id=agent["agent_id"],
                    agent_name=agent["name"],
                    description=agent.get("description"),
                    tags=agent.get("tags", []),
                    icon_url=agent.get("icon_url"),
                    owner_id=agent.get("owner_id"),
                    version=agent.get("version"),
                    score=agent.get("score")
                ))
            
            total_matches = search_result["total"]
            showing = len(agents)
            
            self.log_info("Agent search completed", 
                         query=query, total=total_matches, showing=showing)
            
            return AgentSearchResponse(
                data=agents,
                query=query,
                total_matches=total_matches,
                showing=showing,
                status_code=200,
                message=f"Found {showing} agents matching '{query}'"
            )
            
        except Exception as e:
            await self.handle_service_error("search_agents", e)
    
    # Indexing methods for real-time updates
    async def index_user(self, user_data: dict) -> dict:
        """Index a user for search"""
        try:
            success = await self.redis_search_service.index_user(user_data)
            if success:
                self.log_info(f"Successfully indexed user: {user_data.get('username')}")
                return {"status": "success", "message": f"User {user_data.get('username')} indexed successfully"}
            else:
                self.log_error("Failed to index user - redis search returned false", user_id=user_data.get("id"))
                return {"status": "error", "message": "Failed to index user"}
        except Exception as e:
            self.log_error("Failed to index user", e, user_id=user_data.get("id"))
            return {"status": "error", "message": f"Failed to index user: {str(e)}"}
    
    async def index_agent(self, agent_data: dict) -> bool:
        """Index an agent for search"""
        try:
            return await self.redis_search_service.index_agent(agent_data)
        except Exception as e:
            self.log_error("Failed to index agent", e, agent_id=agent_data.get("agent_id"))
            return False
    
    async def delete_user_from_search(self, user_id: str) -> bool:
        """Remove user from search index"""
        try:
            return await self.redis_search_service.delete_user(user_id)
        except Exception as e:
            self.log_error("Failed to delete user from search", e, user_id=user_id)
            return False
    
    async def delete_agent_from_search(self, agent_id: str) -> bool:
        """Remove agent from search index"""
        try:
            return await self.redis_search_service.delete_agent(agent_id)
        except Exception as e:
            self.log_error("Failed to delete agent from search", e, agent_id=agent_id)
            return False
    
    async def update_agent_in_search(self, agent_id: str) -> bool:
        """Update agent in search index from current registry data"""
        try:
            # Get current agent data from registry
            registry = await self.service.get_registry_by_agent_id(agent_id)
            if not registry:
                self.log_warning(f"Agent {agent_id} not found in registry for search update")
                return False
            
            # Format for search index
            tags = []
            if hasattr(registry, 'tags') and registry.tags:
                tags = registry.tags
            
            agent_data = {
                "agent_id": registry.id,
                "name": registry.name,
                "description": registry.description,
                "tags": tags,
                "icon_url": getattr(registry, 'iconUrl', None),
                "owner_id": getattr(registry, 'owner_id', None),
                "version": registry.version,
                "url": registry.url,
                "created_at": getattr(registry, 'created_at', None),
                "updated_at": getattr(registry, 'updated_at', None)
            }
            
            return await self.redis_search_service.index_agent(agent_data)
            
        except Exception as e:
            self.log_error("Failed to update agent in search", e, agent_id=agent_id)
            return False