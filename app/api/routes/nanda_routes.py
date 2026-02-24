"""
NANDA Routes - NANDA registry API endpoints
"""

from fastapi import APIRouter, Path, Query, Depends
from typing import Optional

from ..handlers import HandlerFactory
from ..types import NANDAApiResponse


def create_nanda_routes(handlers: HandlerFactory) -> APIRouter:
    """Create NANDA-related routes"""
    router = APIRouter(prefix="/nanda", tags=["NANDA Registry"])

    @router.get(
        "/health",
        response_model=NANDAApiResponse,
        summary="NANDA API Health Check",
        description="Check if the NANDA API service is healthy and accessible",
    )
    async def nanda_health_check():
        return await handlers.nanda.health_check()

    @router.get(
        "/agents",
        response_model=NANDAApiResponse,
        summary="Get All Agents",
        description="Retrieve all agents from NANDA registry with optional filtering",
    )
    async def get_all_agents(
        limit: Optional[int] = Query(100, description="Maximum number of agents to return", ge=1, le=10000),
        page: Optional[int] = Query(1, description="Page number for pagination", ge=1),
        agent_type: Optional[str] = Query(None, description="Filter by agent type (all, skill, persona, communication, iot)"),
        status: Optional[str] = Query(None, description="Filter by status (online, offline)"),
        category: Optional[str] = Query(None, description="Filter by category")
    ):
        return await handlers.nanda.get_all_agents(
            limit=limit,
            page=page,
            agent_type=agent_type,
            status=status,
            category=category
        )

    @router.get(
        "/agents/{agent_id}",
        response_model=NANDAApiResponse,
        summary="Get Agent by ID",
        description="Retrieve specific agent details by ID from NANDA registry",
    )
    async def get_agent_by_id(
        agent_id: str = Path(..., description="The unique identifier of the agent")
    ):
        return await handlers.nanda.get_agent_by_id(agent_id)

    @router.get(
        "/agents/search",
        response_model=NANDAApiResponse,
        summary="Search Agents",
        description="Search agents by name or description in NANDA registry",
    )
    async def search_agents(
        query: str = Query(..., description="Search query string", min_length=1, max_length=100),
        limit: Optional[int] = Query(50, description="Maximum number of results", ge=1, le=1000)
    ):
        return await handlers.nanda.search_agents(query=query, limit=limit)

    @router.get(
        "/agents/category/{category}",
        response_model=NANDAApiResponse,
        summary="Get Agents by Category",
        description="Retrieve agents filtered by category from NANDA registry",
    )
    async def get_agents_by_category(
        category: str = Path(..., description="Category to filter by (skill, persona, communication, iot)"),
        limit: Optional[int] = Query(100, description="Maximum number of results", ge=1, le=1000)
    ):
        return await handlers.nanda.get_agents_by_category(category=category, limit=limit)

    @router.get(
        "/agents/online",
        response_model=NANDAApiResponse,
        summary="Get Online Agents",
        description="Retrieve all currently online agents from NANDA registry",
    )
    async def get_online_agents(
        limit: Optional[int] = Query(100, description="Maximum number of results", ge=1, le=1000)
    ):
        return await handlers.nanda.get_online_agents(limit=limit)

    @router.get(
        "/agents/{agent_id}/facts",
        response_model=NANDAApiResponse,
        summary="Get Agent Facts",
        description="Retrieve detailed agent facts and metadata from NANDA registry",
    )
    async def get_agent_facts(
        agent_id: str = Path(..., description="The unique identifier of the agent")
    ):
        return await handlers.nanda.get_agent_facts(agent_id)

    @router.get(
        "/statistics",
        response_model=NANDAApiResponse,
        summary="Get Agent Statistics",
        description="Get aggregate statistics about agents in the NANDA registry",
    )
    async def get_agent_statistics():
        return await handlers.nanda.get_agent_statistics()

    # Messages API Endpoints
    
    @router.get(
        "/messages",
        response_model=NANDAApiResponse,
        summary="Get All Messages",
        description="Retrieve all messages from NANDA registry with optional pagination",
    )
    async def get_all_messages(
        limit: Optional[int] = Query(20, description="Maximum number of messages to return", ge=1, le=1000),
        offset: Optional[int] = Query(None, description="Offset for pagination", ge=0),
        before: Optional[str] = Query(None, description="Get messages before this message ID"),
        after: Optional[str] = Query(None, description="Get messages after this message ID")
    ):
        return await handlers.nanda.get_all_messages(
            limit=limit,
            offset=offset,
            before=before,
            after=after
        )

    @router.get(
        "/messages/agent/{agent_id}",
        response_model=NANDAApiResponse,
        summary="Get Messages by Agent",
        description="Retrieve messages for a specific agent from NANDA registry",
    )
    async def get_messages_by_agent(
        agent_id: str = Path(..., description="The unique identifier of the agent"),
        limit: Optional[int] = Query(20, description="Maximum number of messages to return", ge=1, le=1000)
    ):
        return await handlers.nanda.get_messages_by_agent(agent_id, limit)

    @router.get(
        "/messages/conversation/{conversation_id}",
        response_model=NANDAApiResponse,
        summary="Get Messages by Conversation",
        description="Retrieve messages for a specific conversation from NANDA registry",
    )
    async def get_messages_by_conversation(
        conversation_id: str = Path(..., description="The unique identifier of the conversation"),
        limit: Optional[int] = Query(50, description="Maximum number of messages to return", ge=1, le=1000)
    ):
        return await handlers.nanda.get_messages_by_conversation(conversation_id, limit)

    @router.get(
        "/messages/type/{message_type}",
        response_model=NANDAApiResponse,
        summary="Get Messages by Type",
        description="Retrieve messages filtered by type from NANDA registry",
    )
    async def get_messages_by_type(
        message_type: str = Path(..., description="The type of messages (a2a_response, a2a_send)"),
        limit: Optional[int] = Query(50, description="Maximum number of messages to return", ge=1, le=1000)
    ):
        return await handlers.nanda.get_messages_by_type(message_type, limit)

    @router.get(
        "/messages/statistics",
        response_model=NANDAApiResponse,
        summary="Get Message Statistics",
        description="Get aggregate statistics about messages in the NANDA registry",
    )
    async def get_message_statistics():
        return await handlers.nanda.get_message_statistics()

    return router