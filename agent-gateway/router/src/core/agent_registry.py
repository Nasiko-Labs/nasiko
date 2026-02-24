"""
Agent registry service for fetching and managing agent information.
"""

import logging
import asyncio
from typing import List, Dict, Optional
from functools import lru_cache

import httpx
from router.src.config import settings

logger = logging.getLogger(__name__)


class AgentRegistryError(Exception):
    """Custom exception for agent registry errors."""
    pass


class AgentRegistry:
    """Service for interacting with the agent registry."""
    
    def __init__(self):
        self.timeout = httpx.Timeout(settings.REQUEST_TIMEOUT)
        self._cache: Optional[List[Dict[str, str]]] = None
        self._cache_timestamp: Optional[float] = None
    
    async def fetch_agent_cards(self, token: str, use_cache: bool = True) -> List[Dict[str, str]]:
        """
        Fetch agent cards from the registry.
        
        Args:
            token: Authorization token
            use_cache: Whether to use cached data if available
            
        Returns:
            List of agent card dictionaries
            
        Raises:
            AgentRegistryError: If fetching fails
        """
        if use_cache and self._is_cache_valid():
            logger.info("Using cached agent cards")
            return self._cache
        
        registry_url = f"{settings.NASIKO_BACKEND}/registry/user/agents/info"
        headers = {"Authorization": f"Bearer {token}", "Content-Type": "application/json"}
        
        logger.info(f"Fetching agent cards from {registry_url}")
        
        try:
            async with httpx.AsyncClient(timeout=self.timeout) as client:
                response = await client.get(registry_url, headers=headers)
                response.raise_for_status()
                data = response.json()
                
                self._validate_response(data)
                agent_cards = data["data"]
                
                # Update cache
                import time
                self._cache = agent_cards
                self._cache_timestamp = time.time()
                
                logger.info(f"Successfully fetched {len(agent_cards)} agent cards")
                return agent_cards
                
        except httpx.HTTPStatusError as e:
            error_msg = f"HTTP error fetching agent cards: {e.response.status_code} {e.response.text}"
            logger.error(error_msg)
            raise AgentRegistryError(error_msg) from e
            
        except httpx.RequestError as e:
            error_msg = f"Request error fetching agent cards: {e}"
            logger.error(error_msg)
            raise AgentRegistryError(error_msg) from e
            
        except Exception as e:
            error_msg = f"Unexpected error fetching agent cards: {e}"
            logger.error(error_msg)
            raise AgentRegistryError(error_msg) from e
    
    def _validate_response(self, data: Dict) -> None:
        """Validate the registry response format."""
        if "data" not in data:
            raise ValueError("Invalid registry response: missing 'data' field")
        
        if not isinstance(data["data"], list):
            raise ValueError("Invalid registry response: 'data' field is not a list")
    
    def _is_cache_valid(self) -> bool:
        """Check if cached data is still valid."""
        if self._cache is None or self._cache_timestamp is None:
            return False
        
        import time
        cache_age = time.time() - self._cache_timestamp
        return cache_age < settings.VECTOR_STORE_CACHE_TTL
    
    def clear_cache(self) -> None:
        """Clear the agent cards cache."""
        self._cache = None
        self._cache_timestamp = None
        logger.info("Agent registry cache cleared")
    
    def find_agent_by_name(self, agent_cards: List[Dict[str, str]], agent_name: str) -> Optional[Dict[str, str]]:
        """
        Find an agent by name in the agent cards list.
        
        Args:
            agent_cards: List of agent card dictionaries
            agent_name: Name of the agent to find
            
        Returns:
            Agent card dictionary if found, None otherwise
        """
        for agent_card in agent_cards:
            try:
                if agent_card.get("name") == agent_name:
                    return agent_card
            except Exception as e:
                logger.error(f"Error accessing agent card fields: {e}")
                continue
        
        return None
    
    def get_agent_url(self, agent_cards: List[Dict[str, str]], agent_name: str) -> Optional[str]:
        """
        Get the URL for a specific agent.
        
        Args:
            agent_cards: List of agent card dictionaries
            agent_name: Name of the agent
            
        Returns:
            Agent URL if found, None otherwise
        """
        agent_card = self.find_agent_by_name(agent_cards, agent_name)
        if agent_card:
            return agent_card.get("url", "")
        return None
    
    def get_fallback_agent(self, agent_cards: List[Dict[str, str]]) -> Optional[tuple[str, str]]:
        """
        Get the first available agent with a valid URL as fallback.
        
        Args:
            agent_cards: List of agent card dictionaries
            
        Returns:
            Tuple of (agent_name, agent_url) if found, None otherwise
        """
        for agent_card in agent_cards:
            url = agent_card.get("url", "")
            name = agent_card.get("name", "")
            if url and name:
                return name, url
        return None