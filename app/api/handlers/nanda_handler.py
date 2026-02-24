"""
NANDA API Handler
Handles HTTP requests and responses for NANDA API operations
"""

from typing import Optional, Dict, Any
from fastapi import HTTPException, Query
from .base_handler import BaseHandler
from app.service.nanda_service import NANDAService
from app.api.types import (
    NANDAApiResponse,
    NANDAAgentsListRequest
)


class NANDAHandler(BaseHandler):
    """
    Handler for NANDA API endpoints
    Processes HTTP requests and delegates to service layer
    """
    
    def __init__(self, service, logger):
        super().__init__(service, logger)
        self.nanda_service = NANDAService(logger)
    
    async def get_all_agents(
        self,
        limit: Optional[int] = Query(100, description="Maximum number of agents to return", ge=1, le=10000),
        page: Optional[int] = Query(1, description="Page number for pagination", ge=1),
        agent_type: Optional[str] = Query(None, description="Filter by agent type (all, skill, persona, communication, iot)"),
        status: Optional[str] = Query(None, description="Filter by status (online, offline)"),
        category: Optional[str] = Query(None, description="Filter by category")
    ) -> NANDAApiResponse:
        """
        Get all agents with optional filtering
        
        Args:
            limit: Maximum number of agents to return
            page: Page number for pagination
            agent_type: Type filter
            status: Status filter
            category: Category filter
            
        Returns:
            NANDAApiResponse containing agents list
        """
        try:
            self.log_info("Handling get all agents request", extra={
                "limit": limit,
                "page": page,
                "agent_type": agent_type,
                "status": status,
                "category": category
            })
            
            # Input validation
            if agent_type and agent_type not in ["all", "skill", "persona", "communication", "iot"]:
                raise HTTPException(
                    status_code=400,
                    detail="Invalid agent_type. Must be one of: all, skill, persona, communication, iot"
                )
            
            if status and status not in ["online", "offline"]:
                raise HTTPException(
                    status_code=400,
                    detail="Invalid status. Must be one of: online, offline"
                )
            
            response = await self.nanda_service.get_all_agents(
                limit=limit,
                page=page,
                agent_type=agent_type,
                status=status,
                category=category
            )
            
            if not response.success:
                raise HTTPException(status_code=response.status_code, detail=response.message)
            
            self.log_info("Successfully handled get all agents request")
            return response
            
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Error handling get all agents request", e)
            raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")
    
    async def get_agent_by_id(self, agent_id: str) -> NANDAApiResponse:
        """
        Get specific agent by ID
        
        Args:
            agent_id: The ID of the agent to retrieve
            
        Returns:
            NANDAApiResponse containing agent details
        """
        try:
            self.log_info("Handling get agent by ID request", extra={"agent_id": agent_id})
            
            # Basic input validation
            if not agent_id or not agent_id.strip():
                raise HTTPException(status_code=400, detail="Agent ID is required")
            
            response = await self.nanda_service.get_agent_by_id(agent_id)
            
            if not response.success:
                if response.status_code == 404:
                    raise HTTPException(status_code=404, detail="Agent not found")
                raise HTTPException(status_code=response.status_code, detail=response.message)
            
            self.log_info("Successfully handled get agent by ID request", extra={"agent_id": agent_id})
            return response
            
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Error handling get agent by ID request", e, agent_id=agent_id)
            raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")
    
    async def search_agents(
        self,
        query: str = Query(..., description="Search query string", min_length=1, max_length=100),
        limit: Optional[int] = Query(50, description="Maximum number of results", ge=1, le=1000)
    ) -> NANDAApiResponse:
        """
        Search agents by name or description
        
        Args:
            query: Search query string
            limit: Maximum number of results
            
        Returns:
            NANDAApiResponse containing search results
        """
        try:
            self.log_info("Handling search agents request", extra={"query": query, "limit": limit})
            
            response = await self.nanda_service.search_agents(query, limit)
            
            if not response.success:
                raise HTTPException(status_code=response.status_code, detail=response.message)
            
            self.log_info("Successfully handled search agents request", extra={"query": query})
            return response
            
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Error handling search agents request", e, query=query)
            raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")
    
    async def get_agents_by_category(
        self,
        category: str,
        limit: Optional[int] = Query(100, description="Maximum number of results", ge=1, le=1000)
    ) -> NANDAApiResponse:
        """
        Get agents by category
        
        Args:
            category: Category to filter by
            limit: Maximum number of results
            
        Returns:
            NANDAApiResponse containing filtered agents
        """
        try:
            self.log_info("Handling get agents by category request", extra={"category": category, "limit": limit})
            
            response = await self.nanda_service.get_agents_by_category(category, limit)
            
            if not response.success:
                raise HTTPException(status_code=response.status_code, detail=response.message)
            
            self.log_info("Successfully handled get agents by category request", extra={"category": category})
            return response
            
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Error handling get agents by category request", e, category=category)
            raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")
    
    async def get_online_agents(
        self,
        limit: Optional[int] = Query(100, description="Maximum number of results", ge=1, le=1000)
    ) -> NANDAApiResponse:
        """
        Get all currently online agents
        
        Args:
            limit: Maximum number of results
            
        Returns:
            NANDAApiResponse containing online agents
        """
        try:
            self.log_info("Handling get online agents request", extra={"limit": limit})
            
            response = await self.nanda_service.get_online_agents(limit)
            
            if not response.success:
                raise HTTPException(status_code=response.status_code, detail=response.message)
            
            self.log_info("Successfully handled get online agents request")
            return response
            
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Error handling get online agents request", e)
            raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")
    
    async def get_agent_facts(self, agent_id: str) -> NANDAApiResponse:
        """
        Get detailed agent facts
        
        Args:
            agent_id: The ID of the agent
            
        Returns:
            NANDAApiResponse containing agent facts
        """
        try:
            self.log_info("Handling get agent facts request", extra={"agent_id": agent_id})
            
            # Basic input validation
            if not agent_id or not agent_id.strip():
                raise HTTPException(status_code=400, detail="Agent ID is required")
            
            response = await self.nanda_service.get_agent_facts(agent_id)
            
            if not response.success:
                if response.status_code == 404:
                    raise HTTPException(status_code=404, detail="Agent facts not found")
                raise HTTPException(status_code=response.status_code, detail=response.message)
            
            self.log_info("Successfully handled get agent facts request", extra={"agent_id": agent_id})
            return response
            
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Error handling get agent facts request", e, agent_id=agent_id)
            raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")
    
    async def get_agent_statistics(self) -> NANDAApiResponse:
        """
        Get aggregate statistics about agents
        
        Returns:
            NANDAApiResponse containing statistics
        """
        try:
            self.log_info("Handling get agent statistics request")
            
            response = await self.nanda_service.get_agent_statistics()
            
            if not response.success:
                raise HTTPException(status_code=response.status_code, detail=response.message)
            
            self.log_info("Successfully handled get agent statistics request")
            return response
            
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Error handling get agent statistics request", e)
            raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")
    
    async def health_check(self) -> NANDAApiResponse:
        """
        Check health of NANDA API service
        
        Returns:
            NANDAApiResponse containing health status
        """
        try:
            self.log_info("Handling NANDA API health check request")
            
            response = await self.nanda_service.health_check()
            
            if not response.success:
                raise HTTPException(status_code=response.status_code, detail=response.message)
            
            self.log_info("Successfully handled NANDA API health check request")
            return response
            
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Error handling NANDA API health check request", e)
            raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")
    
    # Messages API Methods
    
    async def get_all_messages(
        self,
        limit: Optional[int] = Query(20, description="Maximum number of messages to return", ge=1, le=1000),
        offset: Optional[int] = Query(None, description="Offset for pagination", ge=0),
        before: Optional[str] = Query(None, description="Get messages before this message ID"),
        after: Optional[str] = Query(None, description="Get messages after this message ID")
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
            self.log_info("Handling get all messages request", extra={
                "limit": limit,
                "offset": offset,
                "before": before,
                "after": after
            })
            
            response = await self.nanda_service.get_all_messages(
                limit=limit,
                offset=offset,
                before=before,
                after=after
            )
            
            if not response.success:
                raise HTTPException(status_code=response.status_code, detail=response.message)
            
            self.log_info("Successfully handled get all messages request")
            return response
            
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Error handling get all messages request", e)
            raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")
    
    async def get_messages_by_agent(
        self,
        agent_id: str,
        limit: Optional[int] = Query(20, description="Maximum number of messages to return", ge=1, le=1000)
    ) -> NANDAApiResponse:
        """
        Get messages for a specific agent
        
        Args:
            agent_id: The ID of the agent
            limit: Maximum number of messages to return
            
        Returns:
            NANDAApiResponse containing agent messages
        """
        try:
            self.log_info("Handling get messages by agent request", extra={"agent_id": agent_id, "limit": limit})
            
            # Basic input validation
            if not agent_id or not agent_id.strip():
                raise HTTPException(status_code=400, detail="Agent ID is required")
            
            response = await self.nanda_service.get_messages_by_agent(agent_id, limit)
            
            if not response.success:
                raise HTTPException(status_code=response.status_code, detail=response.message)
            
            self.log_info("Successfully handled get messages by agent request", extra={"agent_id": agent_id})
            return response
            
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Error handling get messages by agent request", e, agent_id=agent_id)
            raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")
    
    async def get_messages_by_conversation(
        self,
        conversation_id: str,
        limit: Optional[int] = Query(50, description="Maximum number of messages to return", ge=1, le=1000)
    ) -> NANDAApiResponse:
        """
        Get messages for a specific conversation
        
        Args:
            conversation_id: The ID of the conversation
            limit: Maximum number of messages to return
            
        Returns:
            NANDAApiResponse containing conversation messages
        """
        try:
            self.log_info("Handling get messages by conversation request", extra={
                "conversation_id": conversation_id,
                "limit": limit
            })
            
            # Basic input validation
            if not conversation_id or not conversation_id.strip():
                raise HTTPException(status_code=400, detail="Conversation ID is required")
            
            response = await self.nanda_service.get_messages_by_conversation(conversation_id, limit)
            
            if not response.success:
                raise HTTPException(status_code=response.status_code, detail=response.message)
            
            self.log_info("Successfully handled get messages by conversation request", extra={
                "conversation_id": conversation_id
            })
            return response
            
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Error handling get messages by conversation request", e, 
                          conversation_id=conversation_id)
            raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")
    
    async def get_messages_by_type(
        self,
        message_type: str,
        limit: Optional[int] = Query(50, description="Maximum number of messages to return", ge=1, le=1000)
    ) -> NANDAApiResponse:
        """
        Get messages filtered by type
        
        Args:
            message_type: The type of messages to filter by
            limit: Maximum number of messages to return
            
        Returns:
            NANDAApiResponse containing filtered messages
        """
        try:
            self.log_info("Handling get messages by type request", extra={
                "message_type": message_type,
                "limit": limit
            })
            
            response = await self.nanda_service.get_messages_by_type(message_type, limit)
            
            if not response.success:
                raise HTTPException(status_code=response.status_code, detail=response.message)
            
            self.log_info("Successfully handled get messages by type request", extra={
                "message_type": message_type
            })
            return response
            
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Error handling get messages by type request", e, message_type=message_type)
            raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")
    
    async def get_message_statistics(self) -> NANDAApiResponse:
        """
        Get aggregate statistics about messages
        
        Returns:
            NANDAApiResponse containing message statistics
        """
        try:
            self.log_info("Handling get message statistics request")
            
            response = await self.nanda_service.get_message_statistics()
            
            if not response.success:
                raise HTTPException(status_code=response.status_code, detail=response.message)
            
            self.log_info("Successfully handled get message statistics request")
            return response
            
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Error handling get message statistics request", e)
            raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")
    
    async def close(self):
        """Close handler resources"""
        await self.nanda_service.close()