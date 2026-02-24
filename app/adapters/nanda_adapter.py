"""
NANDA API Adapter
Implements the adapter pattern for interacting with NANDA registry API
"""

from typing import Optional, Dict, Any
import httpx
from .base_adapter import BaseAdapter
from app.api.types import (
    NANDAApiResponse, 
    NANDAAgentsResponse, 
    NANDAAgentsListRequest,
    NANDAAgentDetailResponse,
    NANDAAgent,
    NANDAMessagesResponse,
    NANDAMessagesListRequest,
    NANDAMessage
)


class NANDAAdapter(BaseAdapter):
    """
    Adapter for NANDA registry API
    Handles all external API calls to NANDA services
    """
    
    def __init__(self, base_url: str = "https://nest.projectnanda.org", **kwargs):
        super().__init__(base_url, **kwargs)
    
    async def health_check(self) -> NANDAApiResponse:
        """
        Check if NANDA API is healthy
        """
        try:
            response = await self._make_request("GET", "/api/health")
            
            if response.status_code == 200:
                return self._build_success_response(
                    {"status": "healthy"},
                    "NANDA API is healthy"
                )
            else:
                return self._handle_response_error(response)
                
        except Exception as e:
            self.logger.error(f"Health check failed: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Health check failed: {str(e)}",
                status_code=500
            )
    
    async def get_agents(self, request: NANDAAgentsListRequest) -> NANDAApiResponse:
        """
        Get list of agents from NANDA API
        
        Args:
            request: NANDAAgentsListRequest with filtering options
            
        Returns:
            NANDAApiResponse containing NANDAAgentsResponse data
        """
        try:
            # Build query parameters
            params = {}
            if request.type:
                params["type"] = request.type
            if request.limit:
                params["limit"] = request.limit
            if request.page:
                params["page"] = request.page
            if request.status:
                params["status"] = request.status
            if request.category:
                params["category"] = request.category
            if request.search:
                params["search"] = request.search
            
            response = await self._make_request("GET", "/api/agents", params=params)
            
            if response.status_code == 200:
                data = response.json()
                
                # Sanitize Unicode before parsing into Pydantic models
                sanitized_data = self._sanitize_unicode(data)
                
                # Parse response into our types
                agents_response = NANDAAgentsResponse(**sanitized_data)
                
                return self._build_success_response(
                    agents_response.model_dump(),
                    "Agents retrieved successfully"
                )
            else:
                return self._handle_response_error(response)
                
        except Exception as e:
            self.logger.error(f"Failed to get agents: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to retrieve agents: {str(e)}",
                status_code=500
            )
    
    async def get_agent_by_id(self, agent_id: str) -> NANDAApiResponse:
        """
        Get specific agent by ID from NANDA API
        
        Args:
            agent_id: The ID of the agent to retrieve
            
        Returns:
            NANDAApiResponse containing NANDAAgent data
        """
        try:
            response = await self._make_request("GET", f"/api/agents/{agent_id}")
            
            if response.status_code == 200:
                data = response.json()
                
                # Sanitize Unicode before parsing into Pydantic models
                sanitized_data = self._sanitize_unicode(data)
                
                # Parse response into our types
                agent = NANDAAgent(**sanitized_data)
                agent_response = NANDAAgentDetailResponse(agent=agent)
                
                return self._build_success_response(
                    agent_response.model_dump(),
                    "Agent retrieved successfully"
                )
            elif response.status_code == 404:
                return NANDAApiResponse(
                    success=False,
                    data=None,
                    message="Agent not found",
                    status_code=404
                )
            else:
                return self._handle_response_error(response)
                
        except Exception as e:
            self.logger.error(f"Failed to get agent {agent_id}: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to retrieve agent: {str(e)}",
                status_code=500
            )
    
    async def get_agents_by_category(self, category: str, limit: int = 100) -> NANDAApiResponse:
        """
        Get agents filtered by category
        
        Args:
            category: The category to filter by (skill, persona, communication, iot)
            limit: Maximum number of agents to return
            
        Returns:
            NANDAApiResponse containing filtered agents
        """
        request = NANDAAgentsListRequest(
            type="all",
            category=category,
            limit=limit
        )
        return await self.get_agents(request)
    
    async def search_agents(self, search_query: str, limit: int = 50) -> NANDAApiResponse:
        """
        Search agents by name or description
        
        Args:
            search_query: The search term
            limit: Maximum number of results to return
            
        Returns:
            NANDAApiResponse containing search results
        """
        request = NANDAAgentsListRequest(
            type="all",
            search=search_query,
            limit=limit
        )
        return await self.get_agents(request)
    
    async def get_online_agents(self, limit: int = 100) -> NANDAApiResponse:
        """
        Get all currently online agents
        
        Args:
            limit: Maximum number of agents to return
            
        Returns:
            NANDAApiResponse containing online agents
        """
        request = NANDAAgentsListRequest(
            type="all",
            status="online",
            limit=limit
        )
        return await self.get_agents(request)
    
    async def get_agent_facts(self, agent_id: str) -> NANDAApiResponse:
        """
        Get detailed agent facts/metadata
        
        Args:
            agent_id: The ID of the agent
            
        Returns:
            NANDAApiResponse containing agent facts
        """
        try:
            # First get the agent to get the factsUrl
            agent_response = await self.get_agent_by_id(agent_id)
            
            if not agent_response.success:
                return agent_response
            
            agent_data = agent_response.data.get("agent")
            facts_url = agent_data.get("factsUrl")
            
            if not facts_url:
                return NANDAApiResponse(
                    success=False,
                    data=None,
                    message="Agent facts URL not available",
                    status_code=404
                )
            
            # Make request to facts URL
            response = await self._make_request("GET", facts_url.replace(self.base_url, ""))
            
            if response.status_code == 200:
                data = response.json()
                return self._build_success_response(
                    data,
                    "Agent facts retrieved successfully"
                )
            else:
                return self._handle_response_error(response)
                
        except Exception as e:
            self.logger.error(f"Failed to get agent facts for {agent_id}: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to retrieve agent facts: {str(e)}",
                status_code=500
            )
    
    async def get_messages(self, request: NANDAMessagesListRequest) -> NANDAApiResponse:
        """
        Get messages from NANDA API
        
        Args:
            request: NANDAMessagesListRequest with filtering options
            
        Returns:
            NANDAApiResponse containing NANDAMessagesResponse data
        """
        try:
            # Build query parameters
            params = {}
            if request.limit:
                params["limit"] = request.limit
            if request.offset:
                params["offset"] = request.offset
            if request.before:
                params["before"] = request.before
            if request.after:
                params["after"] = request.after
            if request.agent_id:
                params["agent_id"] = request.agent_id
            if request.conversation_id:
                params["conversation_id"] = request.conversation_id
            if request.message_type:
                params["type"] = request.message_type
            
            response = await self._make_request("GET", "/api/messages", params=params)
            
            if response.status_code == 200:
                data = response.json()
                
                # Sanitize Unicode before parsing into Pydantic models
                sanitized_data = self._sanitize_unicode(data)
                
                # Parse response - it's a list directly
                if isinstance(sanitized_data, list):
                    messages = [NANDAMessage(**msg) for msg in sanitized_data]
                    messages_response = NANDAMessagesResponse(
                        messages=messages,
                        total=len(messages),
                        has_more=len(messages) >= (request.limit or 20)
                    )
                else:
                    # If it's wrapped in an object
                    messages = [NANDAMessage(**msg) for msg in sanitized_data.get('messages', sanitized_data)]
                    messages_response = NANDAMessagesResponse(
                        messages=messages,
                        total=sanitized_data.get('total', len(messages)),
                        has_more=sanitized_data.get('has_more', len(messages) >= (request.limit or 20))
                    )
                
                return self._build_success_response(
                    messages_response.model_dump(),
                    "Messages retrieved successfully"
                )
            else:
                return self._handle_response_error(response)
                
        except Exception as e:
            self.logger.error(f"Failed to get messages: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to retrieve messages: {str(e)}",
                status_code=500
            )
    
    async def get_messages_by_agent(self, agent_id: str, limit: int = 20) -> NANDAApiResponse:
        """
        Get messages for a specific agent
        
        Args:
            agent_id: The ID of the agent
            limit: Maximum number of messages to return
            
        Returns:
            NANDAApiResponse containing agent messages
        """
        request = NANDAMessagesListRequest(
            agent_id=agent_id,
            limit=limit
        )
        return await self.get_messages(request)
    
    async def get_messages_by_conversation(self, conversation_id: str, limit: int = 50) -> NANDAApiResponse:
        """
        Get messages for a specific conversation
        
        Args:
            conversation_id: The ID of the conversation
            limit: Maximum number of messages to return
            
        Returns:
            NANDAApiResponse containing conversation messages
        """
        request = NANDAMessagesListRequest(
            conversation_id=conversation_id,
            limit=limit
        )
        return await self.get_messages(request)
    
    async def get_messages_by_type(self, message_type: str, limit: int = 50) -> NANDAApiResponse:
        """
        Get messages filtered by type
        
        Args:
            message_type: The type of messages (a2a_response, a2a_send)
            limit: Maximum number of messages to return
            
        Returns:
            NANDAApiResponse containing filtered messages
        """
        request = NANDAMessagesListRequest(
            message_type=message_type,
            limit=limit
        )
        return await self.get_messages(request)