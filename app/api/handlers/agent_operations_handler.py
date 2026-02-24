"""
Agent Operations Handler - Agent build and deployment operations
"""

from fastapi import HTTPException, status, UploadFile
from fastapi.responses import FileResponse

from .base_handler import BaseHandler
from ..types import AgentBuildRequest, AgentDeployRequest, AgentBuildStatusUpdateRequest, AgentDeploymentStatusUpdateRequest, VersionMappingResponse
from ...entity.entity import AgentBuildInDB, AgentDeploymentBase, BuildStatus, DeploymentStatus
from ...service.agent_operations_service import AgentOperationsService
from ...service.k8s_service import K8sService
from bson import ObjectId
from datetime import datetime, timezone


class AgentOperationsHandler(BaseHandler):
    """Handler for agent build and deployment operations"""

    def __init__(self, service, logger):
        super().__init__(service, logger)
        # Initialize agent operations service directly
        k8s_service = K8sService(logger)
        self.agent_operations_service = AgentOperationsService(service.repo, k8s_service, logger)

    async def create_build_record(self, build_data: AgentBuildStatusUpdateRequest) -> AgentBuildInDB:
        """Create a build record without triggering K8s job (used by k8s build worker)"""
        try:
            self.logger.info(f"HANDLER: Creating build record for agent {build_data.agent_id}")
            result = await self.agent_operations_service.create_build_record_only(build_data)
            return result
        except Exception as e:
            self.logger.error(f"HANDLER: Failed to create build record: {str(e)}")
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Failed to create build record: {str(e)}"
            )

    async def update_build_status(self, build_id: str, status_data: AgentBuildStatusUpdateRequest):
        """Update build status (used by k8s build worker)"""
        try:
            self.logger.info(f"HANDLER: Updating build {build_id} status to {status_data.status}")
            result = await self.agent_operations_service.update_build_status_only(build_id, status_data)
            return result
        except ValueError:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=f"Invalid build ID format: {build_id}"
            )
        except Exception as e:
            self.logger.error(f"HANDLER: Failed to update build status: {str(e)}")
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Failed to update build status: {str(e)}"
            )

    async def create_deployment_record(self, deploy_data: AgentDeploymentStatusUpdateRequest) -> AgentDeploymentBase:
        """Create a deployment record without triggering K8s deployment (used by k8s build worker)"""
        try:
            self.logger.info(f"HANDLER: Creating deployment record for agent {deploy_data.agent_id}")
            result = await self.agent_operations_service.create_deployment_record_only(deploy_data)
            return result
        except Exception as e:
            self.logger.error(f"HANDLER: Failed to create deployment record: {str(e)}")
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Failed to create deployment record: {str(e)}"
            )

    async def update_deployment_status(self, deployment_id: str, status_data: AgentDeploymentStatusUpdateRequest):
        """Update deployment status (used by k8s build worker)"""
        try:
            self.logger.info(f"HANDLER: Updating deployment {deployment_id} status to {status_data.status}")
            result = await self.agent_operations_service.update_deployment_status_only(deployment_id, status_data)
            return result
        except ValueError:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=f"Invalid deployment ID format: {deployment_id}"
            )
        except Exception as e:
            self.logger.error(f"HANDLER: Failed to update deployment status: {str(e)}")
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Failed to update deployment status: {str(e)}"
            )

    async def get_version_mapping(self, agent_id: str, semantic_version: str) -> VersionMappingResponse:
        """Get the Docker image tag for a semantic version of an agent"""
        try:
            self.logger.info(f"HANDLER: Getting version mapping for agent {agent_id}, version {semantic_version}")
            result = await self.agent_operations_service.get_version_mapping(agent_id, semantic_version)
            
            if result is None:
                raise HTTPException(
                    status_code=status.HTTP_404_NOT_FOUND,
                    detail=f"Version mapping not found for agent {agent_id} version {semantic_version}"
                )
            
            return VersionMappingResponse(
                agent_id=agent_id,
                semantic_version=semantic_version,
                image_tag=result["image_tag"],
                timestamp=result["timestamp"],
                status_code=200,
                message="Version mapping retrieved successfully"
            )
        except HTTPException:
            raise
        except Exception as e:
            self.logger.error(f"HANDLER: Failed to get version mapping: {str(e)}")
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Failed to get version mapping: {str(e)}"
            )
