"""
Main Router - Combines all feature-specific routes
"""

from fastapi import APIRouter

from .agent_upload_routes import create_agent_upload_routes
from .agent_operations_routes import create_agent_operations_routes
from .agent_update_routes import create_agent_update_routes
from .chat_history_routes import create_chat_history_routes
from .github_routes import create_github_routes
from .health_routes import create_health_routes
from .n8n_routes import create_n8n_routes
from .registry_routes import create_registry_routes
from .search_routes import create_search_routes
from .superuser_routes import create_superuser_routes
from .observability_routes import create_observability_routes
from .nanda_routes import create_nanda_routes
from ..handlers import HandlerFactory


def create_router(handlers: HandlerFactory, logger) -> APIRouter:
    """
    Create the main API router by combining all feature-specific routes
    """
    router = APIRouter()
    
    # Include organized routes
    router.include_router(create_health_routes(handlers))
    router.include_router(create_registry_routes(handlers))
    router.include_router(create_agent_upload_routes(handlers))
    router.include_router(create_agent_operations_routes(handlers))
    router.include_router(create_agent_update_routes(handlers))
    router.include_router(create_github_routes(handlers))
    router.include_router(create_n8n_routes(handlers))
    router.include_router(create_superuser_routes(handlers))
    router.include_router(create_search_routes(handlers))
    router.include_router(create_chat_history_routes(handlers))
    router.include_router(create_observability_routes(handlers))
    router.include_router(create_nanda_routes(handlers))
    
    return router