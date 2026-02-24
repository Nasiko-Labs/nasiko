"""
Redis Search Service - Handles search indexing and querying for users and agents using Redis
"""

import redis.asyncio as redis
from typing import List, Dict, Any, Optional, Set
import asyncio
import logging
import os
import json
from datetime import datetime
import re

class RedisSearchService:
    """Service for handling Redis-based search operations"""
    
    def __init__(self, logger=None):
        self.logger = logger or logging.getLogger(__name__)
        
        # Get Redis configuration from environment  
        # Default to the Docker service name when running in containers
        redis_host = os.getenv("REDIS_HOST", "localhost")
        redis_url = os.getenv("REDIS_URL", f"redis://{redis_host}:6379")
        
        self.redis = redis.from_url(redis_url)
        
        # Key prefixes for search indexes
        self.users_hash_prefix = "search:user:"
        self.agents_hash_prefix = "search:agent:"
        
        # Search index keys
        self.users_by_username = "search:users:by_username"
        self.users_by_email = "search:users:by_email" 
        self.users_by_role = "search:users:by_role:"
        self.users_active = "search:users:active"
        self.users_all = "search:users:all"
        
        self.agents_by_name = "search:agents:by_name"
        self.agents_by_owner = "search:agents:by_owner:"
        self.agents_by_tag = "search:agents:by_tag:"
        self.agents_by_status = "search:agents:by_status:"
        self.agents_all = "search:agents:all"
        
    async def _check_connection(self) -> bool:
        """Check if Redis is available"""
        try:
            await self.redis.ping()
            return True
        except Exception as e:
            self.logger.error(f"Redis connection failed: {e}")
            return False
    
    async def initialize(self):
        """Initialize Redis search service"""
        if not await self._check_connection():
            self.logger.warning("Redis not available, search functionality will be limited")
            return False
            
        self.logger.info("Redis search service initialized successfully")
        return True
    
    def _serialize_for_redis(self, data: Dict[str, Any]) -> Dict[str, str]:
        """Serialize data for Redis storage (all values must be strings)"""
        serialized = {}
        
        for key, value in data.items():
            if value is None:
                serialized[key] = ""  # Convert None to empty string
            elif isinstance(value, datetime):
                serialized[key] = value.isoformat()  # Convert datetime to ISO string
            elif isinstance(value, (dict, list)):
                serialized[key] = json.dumps(value)  # Convert complex objects to JSON
            elif isinstance(value, bool):
                serialized[key] = str(value).lower()  # Convert bool to "true"/"false"
            else:
                serialized[key] = str(value)  # Convert everything else to string
                
        return serialized
    
    def _deserialize_from_redis(self, data: Dict[str, Any]) -> Dict[str, Any]:
        """Deserialize data from Redis storage"""
        if not data:
            return {}
            
        deserialized = {}
        
        for key, value in data.items():
            # Decode bytes to string if needed
            if isinstance(value, bytes):
                value = value.decode('utf-8')
            
            # Convert empty strings back to None for certain fields
            if value == "" and key in ["avatar_url", "icon_url", "created_at", "updated_at"]:
                deserialized[key] = None
            # Try to parse boolean values
            elif value in ["true", "false"]:
                deserialized[key] = value == "true"
            # Try to parse JSON for complex objects
            elif key == "tags" and value:
                try:
                    deserialized[key] = json.loads(value)
                except:
                    deserialized[key] = []
            else:
                deserialized[key] = value
                
        return deserialized
    
    def _normalize_query(self, query: str) -> str:
        """Normalize search query for consistent matching"""
        return query.lower().strip()
    
    def _create_search_tokens(self, text: str) -> List[str]:
        """Create search tokens for partial matching"""
        if not text:
            return []
        
        text = self._normalize_query(text)
        tokens = []
        
        # Add full text
        tokens.append(text)
        
        # Add words
        words = re.findall(r'\w+', text)
        tokens.extend(words)
        
        # Add prefixes for autocomplete (minimum 2 chars)
        for word in words:
            if len(word) >= 2:
                for i in range(2, len(word) + 1):
                    tokens.append(word[:i])
        
        return list(set(tokens))
    
    async def _calculate_match_score(self, query: str, text: str, boost: float = 1.0) -> float:
        """Calculate match score for ranking"""
        if not text or not query:
            return 0.0
        
        query_norm = self._normalize_query(query)
        text_norm = self._normalize_query(text)
        
        # Exact match gets highest score
        if query_norm == text_norm:
            return 100.0 * boost
        
        # Prefix match
        if text_norm.startswith(query_norm):
            return 90.0 * boost
        
        # Contains match
        if query_norm in text_norm:
            return 70.0 * boost
        
        # Word boundary match
        words = text_norm.split()
        for word in words:
            if word.startswith(query_norm):
                return 60.0 * boost
            if query_norm in word:
                return 50.0 * boost
        
        return 0.0
    
    async def search_users(self, query: str, limit: int = 10) -> Dict[str, Any]:
        """Search for users with fuzzy and prefix matching"""
        try:
            if not await self._check_connection():
                return {"users": [], "total": 0, "error": "Redis unavailable"}
            
            query_norm = self._normalize_query(query)
            if len(query_norm) < 2:
                return {"users": [], "total": 0}
            
            # Get all active users
            active_user_ids = await self.redis.smembers(self.users_active)
            if not active_user_ids:
                return {"users": [], "total": 0}
            
            # Score and collect matching users
            user_scores = []
            
            for user_id_bytes in active_user_ids:
                user_id = user_id_bytes.decode() if isinstance(user_id_bytes, bytes) else user_id_bytes
                
                # Get user data
                user_data = await self.redis.hgetall(f"{self.users_hash_prefix}{user_id}")
                if not user_data:
                    continue
                
                # Decode bytes to strings
                user_data = {
                    k.decode() if isinstance(k, bytes) else k: 
                    v.decode() if isinstance(v, bytes) else v 
                    for k, v in user_data.items()
                }
                
                # Calculate scores for different fields
                username_score = await self._calculate_match_score(query_norm, user_data.get("username", ""), 3.0)
                display_name_score = await self._calculate_match_score(query_norm, user_data.get("display_name", ""), 2.5)
                email_score = await self._calculate_match_score(query_norm, user_data.get("email", ""), 1.5)
                
                total_score = max(username_score, display_name_score, email_score)
                
                if total_score > 0:
                    user_data["score"] = total_score
                    user_scores.append(user_data)
            
            # Sort by score (descending) then by username
            user_scores.sort(key=lambda x: (-x["score"], x.get("username", "")))
            
            # Limit results
            limited_users = user_scores[:limit]
            
            return {
                "users": limited_users,
                "total": len(user_scores),
                "max_score": limited_users[0]["score"] if limited_users else 0
            }
            
        except Exception as e:
            self.logger.error(f"User search failed: {e}")
            return {"users": [], "total": 0, "error": str(e)}
    
    async def search_agents(self, query: str, limit: int = 10) -> Dict[str, Any]:
        """Search for agents with fuzzy and prefix matching"""
        try:
            if not await self._check_connection():
                return {"agents": [], "total": 0, "error": "Redis unavailable"}
            
            query_norm = self._normalize_query(query)
            if len(query_norm) < 2:
                return {"agents": [], "total": 0}
            
            # Get all agents
            all_agent_ids = await self.redis.smembers(self.agents_all)
            if not all_agent_ids:
                return {"agents": [], "total": 0}
            
            # Score and collect matching agents
            agent_scores = []
            
            for agent_id_bytes in all_agent_ids:
                agent_id = agent_id_bytes.decode() if isinstance(agent_id_bytes, bytes) else agent_id_bytes
                
                # Get agent data
                agent_data = await self.redis.hgetall(f"{self.agents_hash_prefix}{agent_id}")
                if not agent_data:
                    continue
                
                # Decode bytes to strings
                agent_data = {
                    k.decode() if isinstance(k, bytes) else k: 
                    v.decode() if isinstance(v, bytes) else v 
                    for k, v in agent_data.items()
                }
                
                # Calculate scores for different fields
                agent_id_score = await self._calculate_match_score(query_norm, agent_data.get("agent_id", ""), 3.0)
                name_score = await self._calculate_match_score(query_norm, agent_data.get("name", ""), 2.8)
                description_score = await self._calculate_match_score(query_norm, agent_data.get("description", ""), 2.0)
                
                # Tag matching (exact match gets high score)
                tag_score = 0.0
                tags_str = agent_data.get("tags", "")
                if tags_str:
                    try:
                        tags = json.loads(tags_str) if isinstance(tags_str, str) else tags_str
                        if isinstance(tags, list):
                            for tag in tags:
                                if query_norm == self._normalize_query(tag):
                                    tag_score = 95.0  # High score for exact tag match
                                    break
                                elif query_norm in self._normalize_query(tag):
                                    tag_score = max(tag_score, 70.0)
                    except:
                        pass
                
                total_score = max(agent_id_score, name_score, description_score, tag_score)
                
                if total_score > 0:
                    # Parse tags for response
                    try:
                        tags = json.loads(agent_data.get("tags", "[]"))
                        agent_data["tags"] = tags if isinstance(tags, list) else []
                    except:
                        agent_data["tags"] = []
                    
                    agent_data["score"] = total_score
                    agent_scores.append(agent_data)
            
            # Sort by score (descending) then by name
            agent_scores.sort(key=lambda x: (-x["score"], x.get("name", "")))
            
            # Limit results
            limited_agents = agent_scores[:limit]
            
            return {
                "agents": limited_agents,
                "total": len(agent_scores),
                "max_score": limited_agents[0]["score"] if limited_agents else 0
            }
            
        except Exception as e:
            self.logger.error(f"Agent search failed: {e}")
            return {"agents": [], "total": 0, "error": str(e)}
    
    async def index_user(self, user_data: Dict[str, Any]) -> bool:
        """Index or update a user document"""
        try:
            if not await self._check_connection():
                return False
            
            user_id = user_data["id"]
            
            # Serialize data for Redis storage
            serialized_data = self._serialize_for_redis(user_data)
            
            # Store user hash
            await self.redis.hset(
                f"{self.users_hash_prefix}{user_id}",
                mapping=serialized_data
            )
            
            # Add to general indexes
            await self.redis.sadd(self.users_all, user_id)
            
            # Index by active status
            if user_data.get("is_active", True):
                await self.redis.sadd(self.users_active, user_id)
            else:
                await self.redis.srem(self.users_active, user_id)
            
            # Index by role
            role = user_data.get("role", "User")
            await self.redis.sadd(f"{self.users_by_role}{role}", user_id)
            
            self.logger.debug(f"Indexed user: {user_id}")
            return True
            
        except Exception as e:
            self.logger.error(f"Failed to index user {user_data.get('id')}: {e}")
            return False
    
    async def index_agent(self, agent_data: Dict[str, Any]) -> bool:
        """Index or update an agent document"""
        try:
            if not await self._check_connection():
                return False
            
            agent_id = agent_data["agent_id"]
            
            # Serialize data for Redis storage (this handles datetime, None, etc.)
            serialized_data = self._serialize_for_redis(agent_data)
            
            # Store agent hash
            await self.redis.hset(
                f"{self.agents_hash_prefix}{agent_id}",
                mapping=serialized_data
            )
            
            # Add to general index
            await self.redis.sadd(self.agents_all, agent_id)
            
            # Index by owner
            if agent_data.get("owner_id"):
                await self.redis.sadd(f"{self.agents_by_owner}{agent_data['owner_id']}", agent_id)
            
            # Index by tags
            tags = agent_data.get("tags", [])
            if isinstance(tags, list):
                for tag in tags:
                    tag_norm = self._normalize_query(tag)
                    await self.redis.sadd(f"{self.agents_by_tag}{tag_norm}", agent_id)
            
            self.logger.debug(f"Indexed agent: {agent_id}")
            return True
            
        except Exception as e:
            self.logger.error(f"Failed to index agent {agent_data.get('agent_id')}: {e}")
            return False
    
    async def delete_user(self, user_id: str) -> bool:
        """Delete a user from the index"""
        try:
            if not await self._check_connection():
                return False
            
            # Get user data before deletion for cleanup
            user_data = await self.redis.hgetall(f"{self.users_hash_prefix}{user_id}")
            
            # Delete user hash
            await self.redis.delete(f"{self.users_hash_prefix}{user_id}")
            
            # Remove from all indexes
            await self.redis.srem(self.users_all, user_id)
            await self.redis.srem(self.users_active, user_id)
            
            # Remove from role index
            if user_data:
                user_data = {
                    k.decode() if isinstance(k, bytes) else k: 
                    v.decode() if isinstance(v, bytes) else v 
                    for k, v in user_data.items()
                }
                role = user_data.get("role", "User")
                await self.redis.srem(f"{self.users_by_role}{role}", user_id)
            
            self.logger.debug(f"Deleted user from index: {user_id}")
            return True
            
        except Exception as e:
            self.logger.error(f"Failed to delete user {user_id}: {e}")
            return False
    
    async def delete_agent(self, agent_id: str) -> bool:
        """Delete an agent from the index"""
        try:
            if not await self._check_connection():
                return False
            
            # Get agent data before deletion for cleanup
            agent_data = await self.redis.hgetall(f"{self.agents_hash_prefix}{agent_id}")
            
            # Delete agent hash
            await self.redis.delete(f"{self.agents_hash_prefix}{agent_id}")
            
            # Remove from all indexes
            await self.redis.srem(self.agents_all, agent_id)
            
            if agent_data:
                agent_data = {
                    k.decode() if isinstance(k, bytes) else k: 
                    v.decode() if isinstance(v, bytes) else v 
                    for k, v in agent_data.items()
                }
                
                # Remove from owner index
                if agent_data.get("owner_id"):
                    await self.redis.srem(f"{self.agents_by_owner}{agent_data['owner_id']}", agent_id)
                
                # Remove from tag indexes
                tags_str = agent_data.get("tags", "[]")
                try:
                    tags = json.loads(tags_str) if isinstance(tags_str, str) else []
                    for tag in tags:
                        tag_norm = self._normalize_query(tag)
                        await self.redis.srem(f"{self.agents_by_tag}{tag_norm}", agent_id)
                except:
                    pass
            
            self.logger.debug(f"Deleted agent from index: {agent_id}")
            return True
            
        except Exception as e:
            self.logger.error(f"Failed to delete agent {agent_id}: {e}")
            return False
    
    async def bulk_index_users(self, users: List[Dict[str, Any]]) -> int:
        """Bulk index multiple users"""
        try:
            if not await self._check_connection() or not users:
                return 0
            
            success_count = 0
            for user in users:
                if await self.index_user(user):
                    success_count += 1
            
            self.logger.info(f"Bulk indexed {success_count} users")
            return success_count
            
        except Exception as e:
            self.logger.error(f"Bulk user indexing failed: {e}")
            return 0
    
    async def bulk_index_agents(self, agents: List[Dict[str, Any]]) -> int:
        """Bulk index multiple agents"""
        try:
            if not await self._check_connection() or not agents:
                return 0
            
            success_count = 0
            for agent in agents:
                if await self.index_agent(agent):
                    success_count += 1
            
            self.logger.info(f"Bulk indexed {success_count} agents")
            return success_count
            
        except Exception as e:
            self.logger.error(f"Bulk agent indexing failed: {e}")
            return 0
    
    async def clear_all_indexes(self) -> bool:
        """Clear all search indexes (for testing/reset)"""
        try:
            if not await self._check_connection():
                return False
            
            # Get all search-related keys
            patterns = [
                "search:user:*",
                "search:agent:*", 
                "search:users:*",
                "search:agents:*"
            ]
            
            for pattern in patterns:
                keys = []
                async for key in self.redis.scan_iter(match=pattern):
                    keys.append(key)
                
                if keys:
                    await self.redis.delete(*keys)
            
            self.logger.info("Cleared all search indexes")
            return True
            
        except Exception as e:
            self.logger.error(f"Failed to clear indexes: {e}")
            return False