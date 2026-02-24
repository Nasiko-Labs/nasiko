"""
Agent Operations Service - Handles agent build and deployment business logic
"""
from typing import Optional
from bson import ObjectId

from ..api.types import AgentBuildRequest, AgentDeployRequest
from ..entity.entity import (
    AgentBuildInDB, 
    AgentBuildBase,
    AgentDeploymentBase,
    BuildStatus,
    DeploymentStatus
)
from ..pkg.config.config import settings


def convert_objectid_to_str(doc: dict) -> dict:
    """Convert ObjectId fields to strings for Pydantic compatibility"""
    if doc and "_id" in doc:
        doc["_id"] = str(doc["_id"])
    return doc


class AgentOperationsService:
    """Service for agent build and deployment operations"""

    def __init__(self, repository, k8s_service, logger):
        self.repo = repository
        self.k8s_service = k8s_service
        self.logger = logger

    async def trigger_agent_build(self, build_request: AgentBuildRequest) -> Optional[AgentBuildInDB]:
        """
        Trigger a BuildKit job to create a Docker image from a Git URL.
        
        1. Clones source from GitHub.
        2. Builds Docker image using BuildKit in K8s.
        3. Pushes image to Harbor registry.
        """
        try:
            self.logger.info(f"SERVICE: Triggering build for agent {build_request.agent_id}")
            
            # 1. Optionally get agent details for naming (commented out for now)
            # agent = await self.repo.get_registry_by_id(ObjectId(build_request.agent_id))

            # 2. Construct the Image Reference
            # Format: <registry_domain>/<image_name>:<tag>
            registry_url = settings.REGISTRY_URL
            image_ref = f"{registry_url}/{build_request.agent_id}:{build_request.version_tag}"

            # 3. Create the Build Record in DB (Status: QUEUED)
            build_data = AgentBuildBase(
                agent_id=build_request.agent_id,
                github_url=build_request.github_url,
                version_tag=build_request.version_tag,
                image_reference=image_ref,
                status=BuildStatus.QUEUED
            )
            
            # Persist to DB
            build_record = await self.repo.create_agent_build(build_data.model_dump())
            if not build_record:
                raise Exception("Failed to save build record to database")
                
            build_id = str(build_record["_id"])

            # 4. Trigger K8s BuildKit Job
            self.logger.info(f"SERVICE: Creating BuildKit job for build {build_id}")
            job_success = self.k8s_service.create_build_job(
                job_id=build_id,
                git_url=build_request.github_url,
                image_destination=image_ref
            )

            # 5. Update Record with Job Status
            if job_success:
                await self.repo.update_agent_build(
                    ObjectId(build_id), 
                    {"status": BuildStatus.BUILDING, "k8s_job_name": f"job-{build_id}"}
                )
                self.logger.info(f"SERVICE: BuildKit job created successfully for build {build_id}")
            else:
                await self.repo.update_agent_build(
                    ObjectId(build_id), 
                    {"status": BuildStatus.FAILED, "logs": "Failed to submit K8s Job"}
                )
                raise Exception("Failed to submit BuildKit job to Kubernetes")

            return AgentBuildInDB(**convert_objectid_to_str(build_record))
            
        except Exception as e:
            self.logger.error(f"SERVICE: Build trigger failed: {str(e)}")
            raise

    async def deploy_agent_container(self, deploy_request: AgentDeployRequest) -> Optional[AgentDeploymentBase]:
        """
        Deploy a previously built agent image to the Kubernetes cluster.
        
        1. Fetches build metadata.
        2. Creates K8s Deployment and Service.
        3. Returns internal service URL.
        """
        try:
            self.logger.info(f"SERVICE: Deploying agent {deploy_request.agent_id} from build {deploy_request.build_id}")
            
            # 1. Fetch the build details to get the image reference
            build_record = await self.repo.get_agent_build_by_id(ObjectId(deploy_request.build_id))
            if not build_record:
                raise ValueError("Build ID not found")
            
            image_ref = build_record["image_reference"]
            self.logger.info(f"SERVICE: Using image {image_ref} for deployment")
            
            # 2. Create Deployment Record in DB
            deployment_data = AgentDeploymentBase(
                agent_id=deploy_request.agent_id,
                build_id=deploy_request.build_id,
                status=DeploymentStatus.STARTING
            )
            
            deploy_record = await self.repo.create_agent_deployment(deployment_data.model_dump())
            deploy_id = str(deploy_record["_id"])
            
            # 3. Trigger K8s Deployment
            # Use deployment ID in the K8s deployment name to ensure uniqueness
            k8s_deploy_name = f"agent-{deploy_id}"
            
            self.logger.info(f"SERVICE: Creating K8s deployment {k8s_deploy_name}")
            k8s_result = self.k8s_service.deploy_agent(
                deployment_name=k8s_deploy_name,
                image_reference=image_ref,
                port=getattr(deploy_request, 'port', 5000),
                env_vars=getattr(deploy_request, 'env_vars', {})
            )

            # 4. Update DB with result
            if k8s_result:
                await self.repo.update_agent_deployment(
                    ObjectId(deploy_id),
                    {
                        "status": DeploymentStatus.RUNNING,
                        "service_url": k8s_result["service_url"]
                    }
                )
                deploy_record["service_url"] = k8s_result["service_url"]
                deploy_record["status"] = DeploymentStatus.RUNNING
                self.logger.info(f"SERVICE: Agent deployed successfully at {k8s_result['service_url']}")
            else:
                await self.repo.update_agent_deployment(
                    ObjectId(deploy_id),
                    {"status": DeploymentStatus.FAILED}
                )
                raise Exception("Failed to create Kubernetes Deployment")

            return AgentDeploymentBase(**convert_objectid_to_str(deploy_record))
            
        except ValueError as e:
            # Re-raise ValueError (like "Build ID not found") without logging as error
            raise
        except Exception as e:
            self.logger.error(f"SERVICE: Deployment failed: {str(e)}")
            raise

    async def create_build_record_only(self, build_data) -> AgentBuildInDB:
        """Create a build record without triggering K8s job (used by k8s build worker)"""
        try:
            from datetime import datetime, timezone
            
            # Create build record data
            build_record_data = {
                "agent_id": build_data.agent_id,
                "github_url": build_data.github_url,
                "version_tag": build_data.version_tag or "latest",
                "image_reference": build_data.image_reference,
                "status": build_data.status,
                "logs": build_data.logs or "",
                "k8s_job_name": build_data.k8s_job_name,
                "created_at": datetime.now(timezone.utc),
                "updated_at": datetime.now(timezone.utc)
            }
            
            # Create in repository
            build_record = await self.repo.create_agent_build(build_record_data)
            if not build_record:
                raise Exception("Failed to create build record")
                
            return AgentBuildInDB(**convert_objectid_to_str(build_record))
            
        except Exception as e:
            self.logger.error(f"SERVICE: Failed to create build record: {str(e)}")
            raise

    async def update_build_status_only(self, build_id: str, status_data):
        """Update build status without K8s operations (used by k8s build worker)"""
        try:
            from datetime import datetime, timezone
            
            # Prepare update data
            update_data = {
                "status": status_data.status,
                "updated_at": datetime.now(timezone.utc)
            }
            
            if status_data.logs:
                update_data["logs"] = status_data.logs
            if status_data.k8s_job_name:
                update_data["k8s_job_name"] = status_data.k8s_job_name
            if status_data.image_reference:
                update_data["image_reference"] = status_data.image_reference
            if status_data.error_message:
                update_data["error_message"] = status_data.error_message
            
            # Update the build record
            updated_record = await self.repo.update_agent_build(ObjectId(build_id), update_data)
            
            if not updated_record:
                raise ValueError(f"Build record with id {build_id} not found")
                
            return {"message": "Build status updated successfully", "build_id": build_id}
            
        except ValueError:
            raise
        except Exception as e:
            self.logger.error(f"SERVICE: Failed to update build status: {str(e)}")
            raise

    async def create_deployment_record_only(self, deploy_data) -> AgentDeploymentBase:
        """Create a deployment record without triggering K8s deployment (used by k8s build worker)"""
        try:
            from datetime import datetime, timezone
            
            # Create deployment record data
            deployment_record_data = {
                "agent_id": deploy_data.agent_id,
                "build_id": deploy_data.build_id,
                "status": deploy_data.status,
                "service_url": deploy_data.service_url,
                "k8s_deployment_name": deploy_data.k8s_deployment_name,
                "namespace": deploy_data.namespace or "nasiko-agents",
                "created_at": datetime.now(timezone.utc),
                "updated_at": datetime.now(timezone.utc)
            }
            
            # Create in repository
            deployment_record = await self.repo.create_agent_deployment(deployment_record_data)
            if not deployment_record:
                raise Exception("Failed to create deployment record")
                
            return AgentDeploymentBase(**convert_objectid_to_str(deployment_record))
            
        except Exception as e:
            self.logger.error(f"SERVICE: Failed to create deployment record: {str(e)}")
            raise

    async def update_deployment_status_only(self, deployment_id: str, status_data):
        """Update deployment status without K8s operations (used by k8s build worker)"""
        try:
            from datetime import datetime, timezone
            
            # Prepare update data
            update_data = {
                "status": status_data.status,
                "updated_at": datetime.now(timezone.utc)
            }
            
            if status_data.service_url:
                update_data["service_url"] = status_data.service_url
            if status_data.k8s_deployment_name:
                update_data["k8s_deployment_name"] = status_data.k8s_deployment_name
            if status_data.namespace:
                update_data["namespace"] = status_data.namespace
            if status_data.error_message:
                update_data["error_message"] = status_data.error_message
            
            # Update the deployment record
            updated_record = await self.repo.update_agent_deployment(ObjectId(deployment_id), update_data)
            
            if not updated_record:
                raise ValueError(f"Deployment record with id {deployment_id} not found")
                
            return {"message": "Deployment status updated successfully", "deployment_id": deployment_id}
            
        except ValueError:
            raise
        except Exception as e:
            self.logger.error(f"SERVICE: Failed to update deployment status: {str(e)}")
            raise
