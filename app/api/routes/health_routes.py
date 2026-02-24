"""
Health Routes - Health check endpoints
"""

from fastapi import APIRouter
from ..handlers import HandlerFactory

def create_health_routes(handlers: HandlerFactory) -> APIRouter:
    """Create health check routes"""
    router = APIRouter(tags=["Health"])

    @router.get(
        "/healthcheck",
        summary="Health Check",
        description="Basic health check endpoint"
    )
    async def healthcheck():
        return await handlers.health.healthcheck()

    return router