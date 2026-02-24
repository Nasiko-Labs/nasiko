"""
Agent Operations Routes - Agent build and deployment endpoints
"""

from fastapi import APIRouter, Query, File, UploadFile, Path

from ..handlers import HandlerFactory
from ..types import AgentBuildRequest, AgentDeployRequest, AgentBuildStatusUpdateRequest, AgentDeploymentStatusUpdateRequest, VersionMappingResponse
from ...entity.entity import AgentBuildInDB, AgentDeploymentBase


def create_agent_operations_routes(handlers: HandlerFactory) -> APIRouter:
    """Create agent operations routes"""
    router = APIRouter(prefix="/agents", tags=["Agent Operations"])

    @router.post(
        "/build",
        response_model=AgentBuildInDB,
        status_code=201,
        summary="Create Build Record",
        description="Create a build record (used by k8s build worker)"
    )
    async def create_build_record(build_data: AgentBuildStatusUpdateRequest):
        return await handlers.agent_operations.create_build_record(build_data)

    @router.post(
        "/deploy",
        response_model=AgentDeploymentBase,
        status_code=201,
        summary="Create Deployment Record",
        description="Create a deployment record (used by k8s build worker)"
    )
    async def create_deployment_record(deploy_data: AgentDeploymentStatusUpdateRequest):
        return await handlers.agent_operations.create_deployment_record(deploy_data)

    @router.put(
        "/build/{build_id}/status",
        summary="Update Build Status",
        description="Update build status (used by k8s build worker)"
    )
    async def update_build_status(build_id: str, status_data: AgentBuildStatusUpdateRequest):
        return await handlers.agent_operations.update_build_status(build_id, status_data)

    @router.put(
        "/deployment/{deployment_id}/status",
        summary="Update Deployment Status",
        description="Update deployment status (used by k8s build worker)"
    )
    async def update_deployment_status(deployment_id: str, status_data: AgentDeploymentStatusUpdateRequest):
        return await handlers.agent_operations.update_deployment_status(deployment_id, status_data)

    @router.get(
        "/build/version-mapping",
        response_model=VersionMappingResponse,
        summary="Get Version Mapping",
        description="Get the Docker image tag for a semantic version of an agent"
    )
    async def get_version_mapping(
        agent_id: str = Query(..., description="Agent ID"),
        semantic_version: str = Query(..., description="Semantic version (e.g., v1.0.0)")
    ):
        return await handlers.agent_operations.get_version_mapping(agent_id, semantic_version)


    return router