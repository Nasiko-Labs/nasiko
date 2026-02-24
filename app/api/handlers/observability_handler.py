from typing import Any, Coroutine

from .base_handler import BaseHandler
from app.service.observability_service import ObservabilityService


class ObservabilityHandler(BaseHandler):
    def __init__(self, service, logger):
        super().__init__(service, logger)
        self.observability_service = ObservabilityService(logger)

    async def get_session_details(self, session_id: str) -> dict[str, Any]:
        """Get session details from Phoenix GraphQL API"""
        return await self.observability_service.get_session_details(session_id)

    async def get_trace_details(self, trace_id: str, project_id: str) -> dict[str, Any]:
        """Get trace details from Phoenix GraphQL API"""
        return await self.observability_service.get_trace_details(trace_id, project_id)

    async def get_span_details(self, span_id: str) -> dict[str, Any]:
        """Get span details from Phoenix GraphQL API"""
        return await self.observability_service.get_span_details(span_id)

    async def get_all_sessions(self, user_id: str, auth_header: str, start_time: str = None) -> dict[str, Any]:
        """Get sessions from all agents/projects the user has access to"""
        return await self.observability_service.get_all_sessions(user_id, auth_header, start_time)

    async def get_agent_project_stats(self, agent_id: str, start_time: str) -> dict[str, Any]:
        """Get project statistics for an agent from Phoenix GraphQL API"""
        return await self.observability_service.get_agent_project_stats(agent_id, start_time)