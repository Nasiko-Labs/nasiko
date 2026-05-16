from typing import Any, Dict, Optional

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

    async def get_all_sessions(
        self, user_id: str, auth_header: str, start_time: str = None
    ) -> dict[str, Any]:
        """Get sessions from all agents/projects the user has access to"""
        return await self.observability_service.get_all_sessions(
            user_id, auth_header, start_time
        )

    async def get_agent_project_stats(
        self, agent_id: str, start_time: str
    ) -> dict[str, Any]:
        """Get project statistics for an agent from Phoenix GraphQL API"""
        return await self.observability_service.get_agent_project_stats(
            agent_id, start_time
        )

    def _map_upload_status(self, status: Optional[str]) -> str:
        if not status:
            return "Unknown"
        status_lower = status.lower()
        if any(term in status_lower for term in ["failed", "error", "cancelled"]):
            return "Failed"
        if any(
            term in status_lower
            for term in ["completed", "deployed", "active", "running"]
        ):
            return "Active"
        return "Setting Up"

    async def _build_agent_name_map(self, agent_ids: list[str]) -> Dict[str, Dict[str, str]]:
        name_map: Dict[str, Dict[str, str]] = {}
        for agent_id in agent_ids:
            try:
                registry = await self.service.get_registry_by_agent_id(agent_id)
                if registry:
                    name_map[agent_id] = {
                        "name": getattr(registry, "name", agent_id),
                        "registry_id": getattr(registry, "id", agent_id),
                    }
                    continue
            except Exception:
                pass
            name_map[agent_id] = {"name": agent_id, "registry_id": agent_id}
        return name_map

    async def _build_deployment_status_map(
        self, user_id: str, agent_ids: list[str]
    ) -> Dict[str, str]:
        status_map: Dict[str, str] = {}
        try:
            upload_statuses = await self.service.get_upload_statuses_by_user(user_id)
            for upload_status in upload_statuses:
                agent_name = upload_status.get("agent_name")
                if not agent_name:
                    continue
                mapped = self._map_upload_status(upload_status.get("status"))
                status_map[agent_name] = mapped
                agent_id = upload_status.get("agent_id")
                if agent_id:
                    status_map[agent_id] = mapped
        except Exception as e:
            self.log_warning(f"Could not load upload statuses for metrics: {e}")

        for agent_id in agent_ids:
            status_map.setdefault(agent_id, "Unknown")
        return status_map

    async def get_agents_metrics(
        self, user_id: str, auth_header: str, hours: int
    ) -> dict[str, Any]:
        """Get per-agent performance metrics enriched with registry and deployment info."""
        metrics_response = await self.observability_service.get_agents_metrics(
            user_id, auth_header, hours
        )
        agents = metrics_response.get("data", {}).get("agents", [])
        agent_ids = [agent["agent_id"] for agent in agents if agent.get("agent_id")]

        name_map = await self._build_agent_name_map(agent_ids)
        deployment_map = await self._build_deployment_status_map(user_id, agent_ids)

        enriched_agents = []
        for agent in agents:
            agent_id = agent.get("agent_id")
            meta = name_map.get(agent_id, {"name": agent_id, "registry_id": agent_id})
            deployment_status = deployment_map.get(agent_id, "Unknown")

            uptime_percent = agent.get("uptime_percent", 0.0)
            if deployment_status == "Failed":
                uptime_percent = 0.0
            elif deployment_status == "Active" and agent.get("total_requests", 0) == 0:
                uptime_percent = max(uptime_percent, 100.0)

            enriched_agents.append(
                {
                    **agent,
                    "name": meta["name"],
                    "deployment_status": deployment_status,
                    "uptime_percent": uptime_percent,
                }
            )

        metrics_response["data"]["agents"] = enriched_agents
        return metrics_response
