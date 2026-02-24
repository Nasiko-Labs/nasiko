"""
Auth Client - Interface to communicate with the auth service
"""
import httpx
import os
from typing import List, Optional
import asyncio

AUTH_SERVICE_URL = os.getenv("AUTH_SERVICE_URL", "http://localhost:8001")

class AuthClient:
    """Client to communicate with the auth service for permission checks"""
    
    def __init__(self):
        self.base_url = AUTH_SERVICE_URL
        self.timeout = 30.0
    
    async def get_user_accessible_agents(self, auth_token: str) -> List[str]:
        """Get list of agent IDs that a user can access using /auth/my-accessible-agents endpoint"""
        if not auth_token:
            raise ValueError("auth_token is required for accessing user's agents")
        
        try:
            # Use the /auth/my-accessible-agents endpoint with token
            headers = {"Authorization": auth_token}
            async with httpx.AsyncClient(timeout=self.timeout) as client:
                response = await client.get(f"{self.base_url}/auth/my-accessible-agents", headers=headers)
                if response.status_code == 200:
                    data = response.json()
                    return data.get("accessible_agents", [])
                return []
        except Exception:
            # If auth service is down, return empty list (fail safely)
            return []
    
    async def get_agents_by_owner(self, owner_id: str) -> List[str]:
        """Get list of agent IDs owned by a user"""
        try:
            async with httpx.AsyncClient(timeout=self.timeout) as client:
                response = await client.get(f"{self.base_url}/auth/agent-permissions/owner/{owner_id}")
                if response.status_code == 200:
                    data = response.json()
                    return [perm.get("agent_id") for perm in data.get("agent_permissions", []) if perm.get("agent_id")]
                return []
        except Exception:
            # If auth service is down, return empty list (fail safely)
            return []
    
    async def create_agent_permissions(self, agent_id: str, owner_id: str) -> bool:
        """Create initial permissions for a newly deployed agent"""
        try:
            async with httpx.AsyncClient(timeout=self.timeout) as client:
                response = await client.post(
                    f"{self.base_url}/auth/agents/{agent_id}/permissions",
                    params={"owner_id": owner_id}
                )
                return response.status_code in [200, 201]
        except Exception:
            # If auth service is down, return False (fail safely)
            return False