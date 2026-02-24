"""
Observability Routes - Phoenix GraphQL Proxy Endpoints
"""

from fastapi import APIRouter, HTTPException, Query, Depends, Request
from typing import Dict, Any, Optional

from ..handlers import HandlerFactory
from ..auth import get_user_id_from_token


def create_observability_routes(handlers: HandlerFactory) -> APIRouter:
    """
    Create observability routes for Phoenix GraphQL proxy endpoints
    """
    router = APIRouter(tags=["observability"], prefix="/observability")

    @router.get("/session/list")
    async def get_all_sessions(
        request: Request,
        user_id: str = Depends(get_user_id_from_token),
        start_time: Optional[str] = Query(None, description="Start time for filtering sessions (ISO datetime format)")
    ) -> Dict[str, Any]:
        """
        Get sessions from all agents/projects the user has access to
        
        Args:
            start_time: Optional ISO datetime string for filtering sessions (e.g., '2025-12-30T19:30:00.000Z')
        
        Returns:
            Aggregated sessions from all user's accessible agents with snake_case field names
        """
        # Get authorization header from request
        auth_header = request.headers.get("authorization")
        if not auth_header:
            raise HTTPException(status_code=401, detail="Authorization header required")
        
        return await handlers.observability.get_all_sessions(user_id, auth_header, start_time)

    @router.get("/session/{session_id}")
    async def get_session_details(
        session_id: str,
        user_id: str = Depends(get_user_id_from_token)
    ) -> Dict[str, Any]:
        """
        Get session details from observability service
        
        Args:
            session_id: Session ID string (e.g., 'ca364f5ebe8845a3a9a7f2cf68948c2f')
        
        Returns:
            Transformed session data with clean traces, cost summary, token usage, and pagination info
        """
        return await handlers.observability.get_session_details(session_id)

    @router.get("/trace/{project_id}/{trace_id}")
    async def get_trace_details(
        project_id: str,
        trace_id: str,
        user_id: str = Depends(get_user_id_from_token)
    ) -> Dict[str, Any]:
        """
        Get trace details from observability service with nested span structure
        
        Args:
            project_id: Project ID (e.g., 'UHJvamVjdDoy')
            trace_id: Trace ID (e.g., 'e56c41d91c7174fe571f45b18f676d7f')
        
        Returns:
            Trace data with nested spans tree, cost summary, latency metrics, and flat span lookup
        """
        return await handlers.observability.get_trace_details(trace_id, project_id)

    @router.get("/span/{span_id}")
    async def get_span_details(
        span_id: str,
        user_id: str = Depends(get_user_id_from_token)
    ) -> Dict[str, Any]:
        """
        Get span details from observability service with parsed JSON attributes
        
        Args:
            span_id: Span node ID (e.g., 'U3Bhbjo1Mw==')
        
        Returns:
            Span details with parsed JSON input/output, attributes, events, annotations, and metrics
        """
        return await handlers.observability.get_span_details(span_id)

    @router.get("/agent/{agent_id}/stats")
    async def get_agent_project_stats(
        agent_id: str,
        start_time: str = Query(..., description="Start time for filtering stats (ISO datetime format)"),
        user_id: str = Depends(get_user_id_from_token)
    ) -> Dict[str, Any]:
        """
        Get project statistics for an agent from observability service
        
        Args:
            agent_id: Agent ID to get project stats for
            start_time: Start time for filtering stats (ISO datetime format, e.g., '2026-01-05T17:30:00.000Z')
        
        Returns:
            Project statistics including trace count, cost summary, latency metrics, and annotation names
        """
        return await handlers.observability.get_agent_project_stats(agent_id, start_time)

    return router