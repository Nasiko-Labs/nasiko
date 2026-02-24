"""
Search Routes - Autocomplete search endpoints for users and agents
"""

from fastapi import APIRouter, Query, Depends
from ..handlers import HandlerFactory
from ..auth import get_user_id_from_token
from ..types import UserSearchResponse, AgentSearchResponse

def create_search_routes(handlers: HandlerFactory) -> APIRouter:
    """Create search-related routes"""
    router = APIRouter(prefix="/search", tags=["Search"])

    @router.get(
        "/users",
        response_model=UserSearchResponse,
        summary="Search Users",
        description="Search for users with autocomplete functionality. Supports prefix matching, fuzzy search, and typo tolerance."
    )
    async def search_users(
        q: str = Query(..., min_length=2, max_length=100, description="Search query (minimum 2 characters)"),
        limit: int = Query(10, le=50, description="Maximum number of results to return"),
        user_id: str = Depends(get_user_id_from_token)
    ):
        """
        Search for users with autocomplete functionality.
        
        Features:
        - Prefix matching: "joh" → "john_doe", "john_smith"
        - Case insensitive: "JOHN" → "john_doe"
        - Fuzzy matching: "jhon" → "john" (typo tolerance)
        - Search in username, display_name, and email
        - Results ranked by relevance
        
        Perfect for autocomplete/typeahead functionality when granting permissions.
        """
        return await handlers.search.search_users(q, limit)

    @router.get(
        "/agents",
        response_model=AgentSearchResponse,
        summary="Search Agents",
        description="Search for agents with autocomplete functionality. Supports prefix matching, fuzzy search, and tag-based search."
    )
    async def search_agents(
        q: str = Query(..., min_length=2, max_length=100, description="Search query (minimum 2 characters)"),
        limit: int = Query(10, le=50, description="Maximum number of results to return"),
        user_id: str = Depends(get_user_id_from_token)
    ):
        """
        Search for agents with autocomplete functionality.
        
        Features:
        - Prefix matching: "trans" → "translator", "transformation-agent"
        - Case insensitive: "TRANSLATOR" → "translator"
        - Fuzzy matching: "translater" → "translator" (typo tolerance)
        - Tag-based search: Search by agent tags
        - Description search: Find agents by description content
        - Results ranked by relevance (exact matches first, then fuzzy)
        
        Perfect for autocomplete/typeahead functionality when selecting agents.
        """
        return await handlers.search.search_agents(q, limit)

    @router.post(
        "/index/user",
        summary="Index User",
        description="Index a user for search (internal system endpoint)"
    )
    async def index_user(user_data: dict):
        """
        Index a user for search (internal system endpoint).
        
        Called by auth service when new users are registered.
        """
        return await handlers.search.index_user(user_data)

    return router