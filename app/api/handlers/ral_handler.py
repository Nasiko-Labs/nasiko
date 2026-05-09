"""
RAL Handler
===========
Handles all RAL-related API requests.  Follows the existing BaseHandler pattern:
thin HTTP layer that delegates all logic to RalService.
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any, Dict

from fastapi import Response
from fastapi.responses import HTMLResponse

from app.service.ral_service import RalService
from .base_handler import BaseHandler

logger = logging.getLogger(__name__)

# Path to the dashboard HTML template
_TEMPLATE_PATH = Path(__file__).parent.parent.parent / "templates" / "ral_dashboard.html"


class RalHandler(BaseHandler):
    """HTTP handler for the Resilient Agent Request Layer API."""

    def __init__(self, service, logger_=None) -> None:
        super().__init__(service, logger_ or logger)
        self.ral_service = RalService(logger_ or logger)

    # ------------------------------------------------------------------
    # Endpoints
    # ------------------------------------------------------------------

    async def get_metrics(self) -> Dict[str, Any]:
        """Return a full RAL metrics snapshot."""
        try:
            snapshot = await self.ral_service.get_metrics_snapshot()
            return {**snapshot, "status_code": 200, "message": "ok"}
        except Exception as exc:
            self.logger.error("RalHandler.get_metrics error: %s", exc)
            return {"status_code": 500, "message": str(exc)}

    async def get_agent_stats(self) -> Dict[str, Any]:
        """Return per-agent traffic stats."""
        try:
            agents = await self.ral_service.get_agent_stats()
            return {"data": agents, "status_code": 200, "message": "ok"}
        except Exception as exc:
            self.logger.error("RalHandler.get_agent_stats error: %s", exc)
            return {"data": [], "status_code": 500, "message": str(exc)}

    async def get_logs(self, limit: int = 50) -> Dict[str, Any]:
        """Return the most recent request log entries."""
        try:
            logs = await self.ral_service.get_request_logs(limit)
            return {"logs": logs, "total": len(logs), "status_code": 200, "message": "ok"}
        except Exception as exc:
            self.logger.error("RalHandler.get_logs error: %s", exc)
            return {"logs": [], "total": 0, "status_code": 500, "message": str(exc)}

    async def flush_cache(self) -> Dict[str, Any]:
        """Flush the entire RAL response cache."""
        try:
            deleted = await self.ral_service.flush_cache()
            return {"deleted": deleted, "status_code": 200, "message": "cache flushed"}
        except Exception as exc:
            self.logger.error("RalHandler.flush_cache error: %s", exc)
            return {"deleted": 0, "status_code": 500, "message": str(exc)}

    async def get_health(self) -> Dict[str, Any]:
        """Return a health summary of the RAL subsystem."""
        try:
            health = await self.ral_service.get_health()
            return {**health, "status_code": 200, "message": "ok"}
        except Exception as exc:
            self.logger.error("RalHandler.get_health error: %s", exc)
            return {"overall": "unhealthy", "status_code": 500, "message": str(exc)}

    async def get_dashboard(self) -> HTMLResponse:
        """Serve the RAL monitoring dashboard HTML page."""
        try:
            html = _TEMPLATE_PATH.read_text(encoding="utf-8")
            return HTMLResponse(content=html, status_code=200)
        except FileNotFoundError:
            self.logger.error("Dashboard template not found at %s", _TEMPLATE_PATH)
            return HTMLResponse(
                content="<h1>Dashboard template not found</h1>",
                status_code=500,
            )
        except Exception as exc:
            self.logger.error("RalHandler.get_dashboard error: %s", exc)
            return HTMLResponse(
                content=f"<h1>Error loading dashboard: {exc}</h1>",
                status_code=500,
            )
