"""
Registry Routes - Agent registry endpoints
"""

from fastapi import APIRouter, Path, Depends, Request

from ..auth import get_user_id_from_token
from ..handlers import HandlerFactory
from ..types import (
    RegistryCreateRequest,
    RegistrySingleResponse,
    SimpleUserAgentsResponse,
    UserAgentsResponse, RegistryUpsertRequest,
    VersionStatusUpdateRequest,
    VersionStatusUpdateResponse,
)

def create_registry_routes(handlers: HandlerFactory) -> APIRouter:
    """Create registry-related routes"""
    router = APIRouter(prefix="/registry", tags=["Agent Registry"])

    @router.post(
        "",
        response_model=RegistrySingleResponse,
        status_code=201,
        summary="Create Agent in the Registry",
        description="Create a new agent entry in the registry",
    )
    async def create_registry(registry_data: RegistryCreateRequest):
        return await handlers.registry.create_registry(registry_data)

    @router.get(
        "/user/agents",
        response_model=SimpleUserAgentsResponse,
        summary="Get My Agents",
        description="Get all agents available to the authenticated user (both uploaded and accessible from registry)",
    )
    async def get_my_agents(
        request: Request, user_id: str = Depends(get_user_id_from_token)
    ):
        return await handlers.registry.get_my_agents(user_id, request)

    @router.get(
        "/user/agents/info",
        response_model=UserAgentsResponse,
        summary="Get info of all agents",
        description="Get info of all agents available to the authenticated user (both uploaded and accessible from registry)",
    )
    async def get_my_agents_info(
        request: Request, user_id: str = Depends(get_user_id_from_token)
    ):
        return await handlers.registry.get_user_agents(user_id, request)

    @router.get(
        "/agent/name/{agent_name}",
        response_model=RegistrySingleResponse,
        summary="Get Registry by Name",
        description="Retrieve a registry entry by agent name",
    )
    async def get_registry_by_name(
        agent_name: str = Path(..., description="Agent name")
    ):
        return await handlers.registry.get_registry_by_name(agent_name)

    @router.get(
        "/agent/id/{agent_id}",
        response_model=RegistrySingleResponse,
        summary="Get Registry by Agent ID",
        description="Retrieve a registry entry by agent ID (requires authentication)",
    )
    async def get_registry_by_agent_id(
        agent_id: str = Path(..., description="Agent ID"),
        user_id: str = Depends(get_user_id_from_token),
    ):
        return await handlers.registry.get_registry_by_agent_id(agent_id)

    @router.put(
        "/agent/{agent_name}",
        response_model=RegistrySingleResponse,
        summary="Upsert Agent in the Registry by Name",
        description="Create or update a registry entry by name",
    )
    async def upsert_registry_by_name(
            upsert_data: RegistryUpsertRequest,
            agent_name: str = Path(..., description="Registry name"),
    ):
        return await handlers.registry.upsert_registry_by_name(agent_name, upsert_data)

    @router.delete(
        "/agent/{agent_id}",
        summary="Delete Agent and All Resources",
        description="Delete an agent and all related resources (K8s deployments, permissions, registry entries, database records)",
    )
    async def delete_agent_completely(
        agent_id: str = Path(..., description="Agent ID to delete"),
        user_id: str = Depends(get_user_id_from_token)
    ):
        return await handlers.registry.delete_agent_completely(agent_id, user_id)

    @router.put(
        "/agent/{agent_name}/version/status",
        response_model=VersionStatusUpdateResponse,
        summary="Update Agent Version Status",
        description="Update the status of an agent version (e.g., from 'building' to 'active')"
    )
    async def update_agent_version_status(
        status_update: VersionStatusUpdateRequest,
        agent_name: str = Path(..., description="Agent name")
    ):
        return await handlers.registry.update_agent_version_status(agent_name, status_update)

    return router
