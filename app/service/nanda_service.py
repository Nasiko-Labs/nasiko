"""
NANDA Service Layer
Handles business logic and orchestrates calls to the NANDA adapter
"""

from typing import Optional, Dict, Any, List
import logging
from app.adapters.nanda_adapter import NANDAAdapter
from app.api.types import (
    NANDAApiResponse,
    NANDAAgentsListRequest,
    NANDAAgent,
    NANDAMessagesListRequest
)


class NANDAService:
    """
    Service layer for NANDA API operations
    Provides business logic and orchestrates adapter calls
    """
    
    def __init__(self, logger: Optional[logging.Logger] = None):
        self.logger = logger or logging.getLogger(__name__)
        self.adapter = NANDAAdapter(logger=self.logger)
    
    async def get_all_agents(
        self, 
        limit: int = 100,
        page: int = 1,
        agent_type: Optional[str] = None,
        status: Optional[str] = None,
        category: Optional[str] = None
    ) -> NANDAApiResponse:
        """
        Get all agents with optional filtering
        
        Args:
            limit: Maximum number of agents to return
            page: Page number for pagination
            agent_type: Type filter (all, skill, persona, communication, iot)
            status: Status filter (online, offline)
            category: Category filter
            
        Returns:
            NANDAApiResponse containing agents list
        """
        try:
            self.logger.info(f"Fetching agents with filters: type={agent_type}, status={status}, category={category}")
            
            request = NANDAAgentsListRequest(
                type=agent_type or "all",
                limit=limit,
                page=page,
                status=status,
                category=category
            )
            
            response = await self.adapter.get_agents(request)
            
            if response.success:
                # Add any business logic processing here
                agents_data = response.data
                self.logger.info(f"Successfully retrieved {len(agents_data.get('agents', []))} agents")
            
            return response
            
        except Exception as e:
            self.logger.error(f"Service error while fetching agents: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to fetch agents: {str(e)}",
                status_code=500
            )
    
    async def get_agent_by_id(self, agent_id: str) -> NANDAApiResponse:
        """
        Get specific agent by ID with business logic validation
        
        Args:
            agent_id: The ID of the agent to retrieve
            
        Returns:
            NANDAApiResponse containing agent details
        """
        try:
            # Input validation
            if not agent_id or not agent_id.strip():
                return NANDAApiResponse(
                    success=False,
                    data=None,
                    message="Agent ID is required",
                    status_code=400
                )
            
            self.logger.info(f"Fetching agent details for ID: {agent_id}")
            
            response = await self.adapter.get_agent_by_id(agent_id)
            
            if response.success:
                # Add any business logic processing here
                agent_data = response.data
                self.logger.info(f"Successfully retrieved agent: {agent_data.get('agent', {}).get('name', 'Unknown')}")
            
            return response
            
        except Exception as e:
            self.logger.error(f"Service error while fetching agent {agent_id}: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to fetch agent: {str(e)}",
                status_code=500
            )
    
    async def search_agents(self, query: str, limit: int = 50) -> NANDAApiResponse:
        """
        Search agents with business logic validation and processing
        
        Args:
            query: Search query string
            limit: Maximum number of results
            
        Returns:
            NANDAApiResponse containing search results
        """
        try:
            # Input validation
            if not query or not query.strip():
                return NANDAApiResponse(
                    success=False,
                    data=None,
                    message="Search query is required",
                    status_code=400
                )
            
            # Sanitize and validate query
            clean_query = query.strip()[:100]  # Limit query length
            
            self.logger.info(f"Searching agents with query: {clean_query}")
            
            response = await self.adapter.search_agents(clean_query, limit)
            
            if response.success:
                # Add any business logic processing here
                agents_data = response.data
                self.logger.info(f"Search returned {len(agents_data.get('agents', []))} results")
            
            return response
            
        except Exception as e:
            self.logger.error(f"Service error while searching agents: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Search failed: {str(e)}",
                status_code=500
            )
    
    async def get_agents_by_category(self, category: str, limit: int = 100) -> NANDAApiResponse:
        """
        Get agents by category with validation
        
        Args:
            category: Category to filter by
            limit: Maximum number of results
            
        Returns:
            NANDAApiResponse containing filtered agents
        """
        try:
            # Validate category
            valid_categories = ["skill", "persona", "communication", "iot"]
            if category not in valid_categories:
                return NANDAApiResponse(
                    success=False,
                    data=None,
                    message=f"Invalid category. Must be one of: {', '.join(valid_categories)}",
                    status_code=400
                )
            
            self.logger.info(f"Fetching agents by category: {category}")
            
            response = await self.adapter.get_agents_by_category(category, limit)
            
            if response.success:
                agents_data = response.data
                self.logger.info(f"Found {len(agents_data.get('agents', []))} agents in category: {category}")
            
            return response
            
        except Exception as e:
            self.logger.error(f"Service error while fetching agents by category {category}: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to fetch agents by category: {str(e)}",
                status_code=500
            )
    
    async def get_online_agents(self, limit: int = 100) -> NANDAApiResponse:
        """
        Get all currently online agents
        
        Args:
            limit: Maximum number of agents to return
            
        Returns:
            NANDAApiResponse containing online agents
        """
        try:
            self.logger.info("Fetching online agents")
            
            response = await self.adapter.get_online_agents(limit)
            
            if response.success:
                agents_data = response.data
                online_count = len(agents_data.get('agents', []))
                self.logger.info(f"Found {online_count} online agents")
            
            return response
            
        except Exception as e:
            self.logger.error(f"Service error while fetching online agents: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to fetch online agents: {str(e)}",
                status_code=500
            )
    
    async def get_agent_facts(self, agent_id: str) -> NANDAApiResponse:
        """
        Get detailed agent facts with validation
        
        Args:
            agent_id: The ID of the agent
            
        Returns:
            NANDAApiResponse containing agent facts
        """
        try:
            # Input validation
            if not agent_id or not agent_id.strip():
                return NANDAApiResponse(
                    success=False,
                    data=None,
                    message="Agent ID is required",
                    status_code=400
                )
            
            self.logger.info(f"Fetching agent facts for ID: {agent_id}")
            
            response = await self.adapter.get_agent_facts(agent_id)
            
            if response.success:
                self.logger.info(f"Successfully retrieved agent facts for: {agent_id}")
            
            return response
            
        except Exception as e:
            self.logger.error(f"Service error while fetching agent facts {agent_id}: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to fetch agent facts: {str(e)}",
                status_code=500
            )
    
    async def get_agent_statistics(self) -> NANDAApiResponse:
        """
        Get aggregate statistics about agents in the NANDA registry
        
        Returns:
            NANDAApiResponse containing statistics
        """
        try:
            self.logger.info("Calculating agent statistics")
            
            # Get all agents to calculate statistics
            all_agents_response = await self.adapter.get_agents(
                NANDAAgentsListRequest(type="all", limit=10000)
            )
            
            if not all_agents_response.success:
                return all_agents_response
            
            agents_data = all_agents_response.data
            agents = agents_data.get('agents', [])
            
            # Calculate statistics
            total_agents = len(agents)
            online_agents = len([a for a in agents if a.get('status') == 'online'])
            offline_agents = total_agents - online_agents
            
            # Category breakdown
            categories = {}
            for agent in agents:
                category = agent.get('category', 'unknown')
                categories[category] = categories.get(category, 0) + 1
            
            # Specialty breakdown
            specialties = {}
            for agent in agents:
                agent_specialties = agent.get('specialties', [])
                for specialty in agent_specialties:
                    specialties[specialty] = specialties.get(specialty, 0) + 1
            
            stats = {
                "total_agents": total_agents,
                "online_agents": online_agents,
                "offline_agents": offline_agents,
                "online_percentage": round((online_agents / total_agents * 100), 2) if total_agents > 0 else 0,
                "categories": categories,
                "top_specialties": dict(sorted(specialties.items(), key=lambda x: x[1], reverse=True)[:10]),
                "pagination": agents_data.get('pagination', {})
            }
            
            self.logger.info(f"Calculated statistics for {total_agents} agents")
            
            return NANDAApiResponse(
                success=True,
                data=stats,
                message="Statistics calculated successfully",
                status_code=200
            )
            
        except Exception as e:
            self.logger.error(f"Service error while calculating statistics: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to calculate statistics: {str(e)}",
                status_code=500
            )
    
    async def health_check(self) -> NANDAApiResponse:
        """
        Check health of NANDA API service
        
        Returns:
            NANDAApiResponse containing health status
        """
        try:
            self.logger.info("Performing NANDA API health check")
            
            response = await self.adapter.health_check()
            
            return response
            
        except Exception as e:
            self.logger.error(f"Health check failed: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Health check failed: {str(e)}",
                status_code=500
            )
    
    # Messages API Methods
    
    async def get_all_messages(
        self,
        limit: int = 20,
        offset: Optional[int] = None,
        before: Optional[str] = None,
        after: Optional[str] = None
    ) -> NANDAApiResponse:
        """
        Get all messages with optional pagination
        
        Args:
            limit: Maximum number of messages to return
            offset: Offset for pagination
            before: Get messages before this message ID
            after: Get messages after this message ID
            
        Returns:
            NANDAApiResponse containing messages list
        """
        try:
            self.logger.info(f"Fetching messages with limit={limit}, offset={offset}")
            
            request = NANDAMessagesListRequest(
                limit=limit,
                offset=offset,
                before=before,
                after=after
            )
            
            response = await self.adapter.get_messages(request)
            
            if response.success:
                messages_data = response.data
                message_count = len(messages_data.get('messages', []))
                self.logger.info(f"Successfully retrieved {message_count} messages")
            
            return response
            
        except Exception as e:
            self.logger.error(f"Service error while fetching messages: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to fetch messages: {str(e)}",
                status_code=500
            )
    
    async def get_messages_by_agent(self, agent_id: str, limit: int = 20) -> NANDAApiResponse:
        """
        Get messages for a specific agent with validation
        
        Args:
            agent_id: The ID of the agent
            limit: Maximum number of messages to return
            
        Returns:
            NANDAApiResponse containing agent messages
        """
        try:
            # Input validation
            if not agent_id or not agent_id.strip():
                return NANDAApiResponse(
                    success=False,
                    data=None,
                    message="Agent ID is required",
                    status_code=400
                )
            
            self.logger.info(f"Fetching messages for agent: {agent_id}")
            
            response = await self.adapter.get_messages_by_agent(agent_id, limit)
            
            if response.success:
                messages_data = response.data
                message_count = len(messages_data.get('messages', []))
                self.logger.info(f"Found {message_count} messages for agent: {agent_id}")
            
            return response
            
        except Exception as e:
            self.logger.error(f"Service error while fetching messages for agent {agent_id}: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to fetch agent messages: {str(e)}",
                status_code=500
            )
    
    async def get_messages_by_conversation(self, conversation_id: str, limit: int = 50) -> NANDAApiResponse:
        """
        Get messages for a specific conversation with validation
        
        Args:
            conversation_id: The ID of the conversation
            limit: Maximum number of messages to return
            
        Returns:
            NANDAApiResponse containing conversation messages
        """
        try:
            # Input validation
            if not conversation_id or not conversation_id.strip():
                return NANDAApiResponse(
                    success=False,
                    data=None,
                    message="Conversation ID is required",
                    status_code=400
                )
            
            self.logger.info(f"Fetching messages for conversation: {conversation_id}")
            
            response = await self.adapter.get_messages_by_conversation(conversation_id, limit)
            
            if response.success:
                messages_data = response.data
                message_count = len(messages_data.get('messages', []))
                self.logger.info(f"Found {message_count} messages for conversation: {conversation_id}")
            
            return response
            
        except Exception as e:
            self.logger.error(f"Service error while fetching messages for conversation {conversation_id}: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to fetch conversation messages: {str(e)}",
                status_code=500
            )
    
    async def get_messages_by_type(self, message_type: str, limit: int = 50) -> NANDAApiResponse:
        """
        Get messages filtered by type with validation
        
        Args:
            message_type: The type of messages to filter by
            limit: Maximum number of messages to return
            
        Returns:
            NANDAApiResponse containing filtered messages
        """
        try:
            # Validate message type
            valid_types = ["a2a_response", "a2a_send"]
            if message_type not in valid_types:
                return NANDAApiResponse(
                    success=False,
                    data=None,
                    message=f"Invalid message type. Must be one of: {', '.join(valid_types)}",
                    status_code=400
                )
            
            self.logger.info(f"Fetching messages by type: {message_type}")
            
            response = await self.adapter.get_messages_by_type(message_type, limit)
            
            if response.success:
                messages_data = response.data
                message_count = len(messages_data.get('messages', []))
                self.logger.info(f"Found {message_count} messages of type: {message_type}")
            
            return response
            
        except Exception as e:
            self.logger.error(f"Service error while fetching messages by type {message_type}: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to fetch messages by type: {str(e)}",
                status_code=500
            )
    
    async def get_message_statistics(self) -> NANDAApiResponse:
        """
        Get aggregate statistics about messages in the NANDA registry
        
        Returns:
            NANDAApiResponse containing message statistics
        """
        try:
            self.logger.info("Calculating message statistics")
            
            # Get recent messages to analyze
            recent_messages_response = await self.adapter.get_messages(
                NANDAMessagesListRequest(limit=1000)  # Sample recent messages
            )
            
            if not recent_messages_response.success:
                return recent_messages_response
            
            messages_data = recent_messages_response.data
            messages = messages_data.get('messages', [])
            
            # Calculate statistics
            total_messages = len(messages)
            
            # Message type breakdown
            message_types = {}
            for message in messages:
                msg_type = message.get('type', 'unknown')
                message_types[msg_type] = message_types.get(msg_type, 0) + 1
            
            # Agent activity
            agent_activity = {}
            for message in messages:
                agent_id = message.get('agent_id') or message.get('from_agent')
                if agent_id:
                    agent_activity[agent_id] = agent_activity.get(agent_id, 0) + 1
            
            # Most active agents (top 10)
            top_agents = dict(sorted(agent_activity.items(), key=lambda x: x[1], reverse=True)[:10])
            
            # Region activity
            region_activity = {}
            for message in messages:
                from_region = message.get('from_region')
                if from_region:
                    region_activity[from_region] = region_activity.get(from_region, 0) + 1
            
            stats = {
                "total_messages_analyzed": total_messages,
                "message_types": message_types,
                "top_active_agents": top_agents,
                "region_activity": region_activity,
                "analysis_note": f"Statistics based on last {total_messages} messages"
            }
            
            self.logger.info(f"Calculated statistics for {total_messages} messages")
            
            return NANDAApiResponse(
                success=True,
                data=stats,
                message="Message statistics calculated successfully",
                status_code=200
            )
            
        except Exception as e:
            self.logger.error(f"Service error while calculating message statistics: {str(e)}")
            return NANDAApiResponse(
                success=False,
                data=None,
                message=f"Failed to calculate message statistics: {str(e)}",
                status_code=500
            )
    
    async def close(self):
        """Close adapter connections"""
        await self.adapter.close()