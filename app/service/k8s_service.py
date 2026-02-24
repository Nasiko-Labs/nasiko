import logging
from typing import Optional, Dict, Any
from kubernetes import client, config
from app.pkg.config.config import settings

class K8sService:
    """
    Service to manage Kubernetes resources for Agent Building and Deployment.
    """

    def __init__(self, logger: logging.Logger):
        self.logger = logger
        self.enabled = settings.K8S_ENABLED
        
        # Load cluster configuration
        # If running locally (dev), use load_kube_config(). 
        # If running inside a pod (prod), use load_incluster_config().
        if not self.enabled:
            self.logger.warning("K8sService disabled via K8S_ENABLED=false")
            self.batch_api = None
            self.apps_api = None
            self.core_api = None
            self.NAMESPACE = "nasiko-agents"
            self.BUILDKIT_ADDR = settings.BUILDKIT_ADDRESS
            self.SECRET_NAME = "agent-registry-credentials"
            self.NODEPORT_SECRET_NAME = "agent-registry-credentials-nodeport"
            return
        try:
            config.load_incluster_config()
            self.logger.info("Loaded in-cluster Kubernetes config")
        except config.ConfigException:
            try:
                config.load_kube_config()
                self.logger.info("Loaded local kubeconfig")
            except config.ConfigException as exc:
                self.logger.warning(
                    "K8s config not available; disabling K8sService. "
                    "Set K8S_ENABLED=false to silence this. Error: %s",
                    exc,
                )
                self.enabled = False
                self.batch_api = None
                self.apps_api = None
                self.core_api = None
                self.NAMESPACE = "nasiko-agents"
                self.BUILDKIT_ADDR = settings.BUILDKIT_ADDRESS
                self.SECRET_NAME = "agent-registry-credentials"
                self.NODEPORT_SECRET_NAME = "agent-registry-credentials-nodeport"
                return

        self.batch_api = client.BatchV1Api()
        self.apps_api = client.AppsV1Api()
        self.core_api = client.CoreV1Api()
        
        # Configuration Constants
        self.NAMESPACE = "nasiko-agents"
        self.BUILDKIT_ADDR = settings.BUILDKIT_ADDRESS  # e.g. tcp://buildkitd.buildkit.svc.cluster.local:1234
        self.SECRET_NAME = "agent-registry-credentials"
        self.NODEPORT_SECRET_NAME = "agent-registry-credentials-nodeport"

    def _ensure_enabled(self):
        if not self.enabled:
            raise RuntimeError("K8sService is disabled (K8S_ENABLED=false or no kubeconfig).")

    def _is_harbor_registry(self, registry_url):
        """Check if the registry URL indicates Harbor internal service"""
        return "harbor-registry.harbor.svc.cluster.local" in registry_url
    
    def _get_buildctl_command(self, image_destination):
        """Generate buildctl command with conditional insecure registry config"""
        registry_url = image_destination.split('/')[0]  # Extract registry from image destination
        
        # Base buildctl command
        base_cmd = [
            "buildctl", "build",
            "--frontend", "dockerfile.v0",
            "--local", "context=/workspace",
            "--local", "dockerfile=/workspace"
        ]
        
        # Add registry-specific flags
        if self._is_harbor_registry(registry_url):
            # Harbor internal registry - add insecure registry flag
            output_arg = f"type=image,name={image_destination},push=true,registry.insecure=true"
        else:
            # External registry (DigitalOcean, AWS ECR, etc.) - use standard HTTPS
            output_arg = f"type=image,name={image_destination},push=true"
        
        return base_cmd + ["--output", output_arg]

    def create_build_job(self, job_id: str, git_url: str, image_destination: str) -> bool:
        """
        Creates a K8s Job to build an image from a Git URL.
        
        Args:
            job_id: Unique identifier for the job (e.g., 'build-agent-xyz-123')
            git_url: HTTPS URL of the git repository
            image_destination: Full tag to push to (e.g., registry.nasiko.io/library/agent:v1)
        """
        self._ensure_enabled()
        try:
            job_name = f"job-{job_id}"
            
            # 1. Define the Shared Volume (Workspace)
            # This volume is shared between the git-clone init container and the buildkit client
            workspace_volume = client.V1Volume(
                name="workspace",
                empty_dir=client.V1EmptyDirVolumeSource()
            )

            # 2. Define Secret Volume for Harbor Auth
            # We mount the docker-registry secret so buildctl can read config.json to authenticate push
            auth_volume = client.V1Volume(
                name="harbor-auth",
                secret=client.V1SecretVolumeSource(
                    secret_name=self.SECRET_NAME,
                    items=[client.V1KeyToPath(key=".dockerconfigjson", path="config.json")]
                )
            )

            # 3. Init Container: Git Clone
            init_container = client.V1Container(
                name="git-clone",
                image="alpine/git:latest",
                command=["git", "clone", git_url, "/workspace"],
                volume_mounts=[
                    client.V1VolumeMount(name="workspace", mount_path="/workspace")
                ]
            )

            # 4. Main Container: Buildkit Client (buildctl)
            # This container sends the content of /workspace to the remote Buildkit Daemon
            main_container = client.V1Container(
                name="buildkit-client",
                image="moby/buildkit:master-rootless", # Use client image
                env=[
                    client.V1EnvVar(name="BUILDKIT_HOST", value=self.BUILDKIT_ADDR)
                ],
                command=self._get_buildctl_command(image_destination),
                volume_mounts=[
                    client.V1VolumeMount(name="workspace", mount_path="/workspace"),
                    # Mount auth config to default docker location
                    client.V1VolumeMount(name="harbor-auth", mount_path="/home/user/.docker/config.json", sub_path="config.json")
                ]
            )

            # 5. Construct the Job Spec
            job = client.V1Job(
                api_version="batch/v1",
                kind="Job",
                metadata=client.V1ObjectMeta(name=job_name, namespace=self.NAMESPACE),
                spec=client.V1JobSpec(
                    ttl_seconds_after_finished=3600, # Auto-delete job 1 hour after finish
                    backoff_limit=1, # Retry once if failed
                    template=client.V1PodTemplateSpec(
                        spec=client.V1PodSpec(
                            restart_policy="Never",
                            init_containers=[init_container],
                            containers=[main_container],
                            volumes=[workspace_volume, auth_volume],
                            security_context=client.V1PodSecurityContext(
                                run_as_user=1000,
                                fs_group=1000
                            )
                        )
                    )
                )
            )

            self.logger.info(f"Submitting Build Job {job_name} to K8s...")
            self.batch_api.create_namespaced_job(namespace=self.NAMESPACE, body=job)
            return True

        except client.exceptions.ApiException as e:
            self.logger.error(f"K8s API Error creating build job: {e}")
            return False

    def deploy_agent(self, deployment_name: str, image_reference: str, port: int = 5000, env_vars: Optional[Dict[str, str]] = None) -> Optional[Dict[str, Any]]:
        """
        Creates a Deployment and Service for the Agent.

        Args:
            deployment_name: Name for the deployment and service
            image_reference: Full image reference (registry/image:tag)
            port: Port the agent listens on (default: 5000, matches agent Dockerfile implementations)
            env_vars: Optional dictionary of environment variables to set
        """
        self._ensure_enabled()
        try:
            app_label = {"app": deployment_name}

            # Convert Harbor cluster DNS to NodePort for Docker Desktop compatibility
            deployment_image = image_reference
            secret_name = self.SECRET_NAME
            
            if "harbor-registry.harbor.svc.cluster.local:5000" in image_reference:
                # Convert to NodePort address for Docker daemon compatibility
                deployment_image = image_reference.replace(
                    "harbor-registry.harbor.svc.cluster.local:5000",
                    "localhost:30500"
                )
                secret_name = self.NODEPORT_SECRET_NAME
                self.logger.info(f"Converted image reference for Docker Desktop: {deployment_image}")

            # Build environment variables list
            env_list = [client.V1EnvVar(name="PORT", value=str(port))]
            if env_vars:
                for key, value in env_vars.items():
                    env_list.append(client.V1EnvVar(name=key, value=value))

            # --- 1. Deployment ---
            deployment = client.V1Deployment(
                api_version="apps/v1",
                kind="Deployment",
                metadata=client.V1ObjectMeta(name=deployment_name, namespace=self.NAMESPACE),
                spec=client.V1DeploymentSpec(
                    replicas=1,
                    selector=client.V1LabelSelector(match_labels=app_label),
                    template=client.V1PodTemplateSpec(
                        metadata=client.V1ObjectMeta(labels=app_label),
                        spec=client.V1PodSpec(
                            # Use NodePort secret for Harbor images, regular secret for others
                            image_pull_secrets=[client.V1LocalObjectReference(name=secret_name)],
                            containers=[
                                client.V1Container(
                                    name="agent",
                                    image=deployment_image,
                                    ports=[client.V1ContainerPort(container_port=port)],
                                    env=env_list
                                )
                            ]
                        )
                    )
                )
            )

            self.logger.info(f"Creating Deployment {deployment_name}...")
            self.apps_api.create_namespaced_deployment(namespace=self.NAMESPACE, body=deployment)

            # --- 2. Service ---
            service = client.V1Service(
                api_version="v1",
                kind="Service",
                metadata=client.V1ObjectMeta(name=deployment_name, namespace=self.NAMESPACE),
                spec=client.V1ServiceSpec(
                    selector=app_label,
                    ports=[client.V1ServicePort(port=80, target_port=port)],
                    type="ClusterIP"
                )
            )

            self.logger.info(f"Creating Service {deployment_name}...")
            self.core_api.create_namespaced_service(namespace=self.NAMESPACE, body=service)

            # Construct the internal DNS URL
            internal_url = f"http://{deployment_name}.{self.NAMESPACE}.svc.cluster.local"
            
            return {
                "deployment_name": deployment_name,
                "service_url": internal_url,
                "status": "provisioned"
            }

        except client.exceptions.ApiException as e:
            # Handle "AlreadyExists" gracefully
            if e.status == 409:
                self.logger.warning(f"Resource {deployment_name} already exists.")
                # You might want to trigger an update (patch) here instead
                return None
            
            self.logger.error(f"K8s API Error deploying agent: {e}")
            return None

    def get_job_status(self, job_name: str) -> str:
        """
        Check the status of the build job.
        Returns: active, succeeded, failed, or unknown
        """
        self._ensure_enabled()
        try:
            job = self.batch_api.read_namespaced_job(job_name, self.NAMESPACE)
            if job.status.succeeded:
                return "succeeded"
            elif job.status.failed:
                return "failed"
            elif job.status.active:
                return "active"
            return "pending"
        except client.exceptions.ApiException:
            return "unknown"

    def create_build_job_from_upload(self, job_id: str, agent_name: str, image_destination: str, backend_url: str = "http://nasiko-backend.nasiko.svc.cluster.local:8000", agent_path: str = None, local_files_path: str = None) -> bool:
        """
        Creates a K8s Job to build an image from uploaded agent files.

        This method is used for zip/directory uploads where files are stored in the backend's /app/agents/{agent_name} directory.
        The build job downloads the agent files from the backend and builds them using BuildKit.
        If local_files_path is provided, uses those files instead of downloading from backend.

        Args:
            job_id: Unique identifier for the job
            agent_name: Name of the agent (used to fetch files from backend)
            image_destination: Full tag to push to (e.g., registry.nasiko.io/library/agent:v1)
            backend_url: URL of the backend service to download agent files from
            agent_path: Path for versioned downloads from backend
            local_files_path: Optional path to local tarball to use instead of downloading
        """
        self._ensure_enabled()
        try:
            job_name = f"job-{job_id}"

            # 1. Define the Shared Volume (Workspace)
            workspace_volume = client.V1Volume(
                name="workspace",
                empty_dir=client.V1EmptyDirVolumeSource()
            )

            # 2. Define Secret Volume for Registry Auth
            auth_volume = client.V1Volume(
                name="harbor-auth",
                secret=client.V1SecretVolumeSource(
                    secret_name=self.SECRET_NAME,
                    items=[client.V1KeyToPath(key=".dockerconfigjson", path="config.json")]
                )
            )

            # 3. Init Container: Use local files if provided, otherwise download from backend
            volumes_list = [workspace_volume, auth_volume]
            
            if local_files_path:
                # Check if this is a ConfigMap name (starts with 'agent-files-') or a local path
                if local_files_path.startswith('agent-files-'):
                    # Use ConfigMap with observability-injected files
                    self.logger.info(f"Using observability-injected files from ConfigMap: {local_files_path}")
                    
                    # Mount the ConfigMap as a volume
                    configmap_volume = client.V1Volume(
                        name="agent-files-cm",
                        config_map=client.V1ConfigMapVolumeSource(name=local_files_path)
                    )
                    volumes_list.append(configmap_volume)
                    
                    init_container = client.V1Container(
                        name="copy-configmap-files",
                        image="busybox:latest",
                        command=[
                            "sh", "-c",
                            # Decode base64 files from ConfigMap and extract to workspace
                            """
                            cd /configmap-files
                            for file in *; do
                                if [ -f "$file" ]; then
                                    # Convert ConfigMap key back to file path
                                    # The encoding is now base64 with special character replacements
                                    # Reverse the character replacements and decode base64
                                    encoded_path=$(echo "$file" | sed 's/_eq_/=/g; s/_plus_/+/g; s/_slash_/\\//g')
                                    target_path=$(echo "$encoded_path" | base64 -d)
                                    
                                    full_target_path="/workspace/$target_path"
                                    target_dir=$(dirname "$full_target_path")
                                    mkdir -p "$target_dir"
                                    base64 -d "$file" > "$full_target_path"
                                    echo "Extracted $file to $full_target_path"
                                fi
                            done
                            echo 'Decoded and extracted observability-injected files to workspace'
                            """
                        ],
                        volume_mounts=[
                            client.V1VolumeMount(name="workspace", mount_path="/workspace"),
                            client.V1VolumeMount(name="agent-files-cm", mount_path="/configmap-files", read_only=True)
                        ]
                    )
                else:
                    # Use local files (original HostPath behavior for backward compatibility)
                    self.logger.info(f"Using local files from: {local_files_path}")
                    
                    # Mount the local files directory as a volume
                    local_files_volume = client.V1Volume(
                        name="local-files",
                        host_path=client.V1HostPathVolumeSource(path=local_files_path)
                    )
                    volumes_list.append(local_files_volume)
                    
                    init_container = client.V1Container(
                        name="copy-local-files",
                        image="busybox:latest",
                        command=[
                            "sh", "-c",
                            "cp -r /local-files/* /workspace/ && echo 'Copied local files to workspace'"
                        ],
                        volume_mounts=[
                            client.V1VolumeMount(name="workspace", mount_path="/workspace"),
                            client.V1VolumeMount(name="local-files", mount_path="/local-files", read_only=True)
                        ]
                    )
            else:
                # Download from backend (original behavior)
                version_param = ""
                if agent_path and '/v' in agent_path:
                    # Extract version from path like /app/agents/agent_name/v1.0.0
                    version = agent_path.split('/v')[-1]
                    version_param = f"?version={version}"
                    self.logger.info(f"Using versioned download URL with version: {version}")
                
                download_url = f"{backend_url}/api/v1/agents/{agent_name}/download{version_param}"
                self.logger.info(f"Downloading agent files from backend: {download_url}")
                
                init_container = client.V1Container(
                    name="download-agent",
                    image="curlimages/curl:latest",
                    command=[
                        "sh", "-c",
                        f"curl -f -o /tmp/agent.tar.gz '{download_url}' && "
                        f"cd /workspace && tar -xzf /tmp/agent.tar.gz && rm /tmp/agent.tar.gz"
                    ],
                    volume_mounts=[
                        client.V1VolumeMount(name="workspace", mount_path="/workspace")
                    ]
                )

            # 4. Main Container: Buildkit Client (buildctl)
            main_container = client.V1Container(
                name="buildkit-client",
                image="moby/buildkit:master-rootless",
                env=[
                    client.V1EnvVar(name="BUILDKIT_HOST", value=self.BUILDKIT_ADDR)
                ],
                command=self._get_buildctl_command(image_destination),
                volume_mounts=[
                    client.V1VolumeMount(name="workspace", mount_path="/workspace"),
                    client.V1VolumeMount(name="harbor-auth", mount_path="/home/user/.docker/config.json", sub_path="config.json")
                ]
            )

            # 5. Construct the Job Spec
            job = client.V1Job(
                api_version="batch/v1",
                kind="Job",
                metadata=client.V1ObjectMeta(name=job_name, namespace=self.NAMESPACE),
                spec=client.V1JobSpec(
                    ttl_seconds_after_finished=3600,
                    backoff_limit=1,
                    template=client.V1PodTemplateSpec(
                        spec=client.V1PodSpec(
                            restart_policy="Never",
                            init_containers=[init_container],
                            containers=[main_container],
                            volumes=volumes_list,
                            security_context=client.V1PodSecurityContext(
                                run_as_user=1000,
                                fs_group=1000
                            )
                        )
                    )
                )
            )

            self.logger.info(f"Submitting Build Job (from upload) {job_name} to K8s...")
            self.batch_api.create_namespaced_job(namespace=self.NAMESPACE, body=job)
            return True

        except client.exceptions.ApiException as e:
            self.logger.error(f"K8s API Error creating build job from upload: {e}")
            return False

    def list_agent_deployments(self, agent_id: str) -> list:
        """
        List all deployments for a given agent ID.
        
        Args:
            agent_id: The agent identifier
            
        Returns:
            List of deployment names that match the agent pattern
        """
        try:
            # List all deployments in the namespace
            deployments = self.apps_api.list_namespaced_deployment(namespace=self.NAMESPACE)
            
            agent_deployments = []
            for deployment in deployments.items:
                deployment_name = deployment.metadata.name
                # K8s deployments are typically named like: agent-{agent_name}-{timestamp}
                if deployment_name.startswith(f"agent-{agent_id}-"):
                    agent_deployments.append(deployment_name)
            
            self.logger.info(f"Found {len(agent_deployments)} deployments for agent {agent_id}")
            return agent_deployments
            
        except client.exceptions.ApiException as e:
            self.logger.error(f"K8s API Error listing deployments for agent {agent_id}: {e}")
            return []
        except Exception as e:
            self.logger.error(f"Error listing deployments for agent {agent_id}: {e}")
            return []

    def delete_agent_deployment(self, deployment_name: str) -> bool:
        """
        Delete a Kubernetes deployment and its associated service.
        
        Args:
            deployment_name: Name of the deployment to delete
            
        Returns:
            True if deletion was successful, False otherwise
        """
        try:
            deletion_success = True
            
            # Delete the deployment
            try:
                self.apps_api.delete_namespaced_deployment(
                    name=deployment_name, 
                    namespace=self.NAMESPACE,
                    propagation_policy='Background'
                )
                self.logger.info(f"Deleted K8s deployment: {deployment_name}")
            except client.exceptions.ApiException as e:
                if e.status == 404:
                    self.logger.info(f"Deployment {deployment_name} not found (already deleted)")
                else:
                    self.logger.error(f"Error deleting deployment {deployment_name}: {e}")
                    deletion_success = False
            
            # Delete the associated service (services are typically named the same as deployments)
            try:
                self.core_api.delete_namespaced_service(
                    name=deployment_name, 
                    namespace=self.NAMESPACE
                )
                self.logger.info(f"Deleted K8s service: {deployment_name}")
            except client.exceptions.ApiException as e:
                if e.status == 404:
                    self.logger.info(f"Service {deployment_name} not found (already deleted or doesn't exist)")
                else:
                    self.logger.error(f"Error deleting service {deployment_name}: {e}")
                    # Don't mark as failure - service might not exist
            
            return deletion_success
            
        except Exception as e:
            self.logger.error(f"Error deleting agent deployment {deployment_name}: {e}")
            return False

    def create_configmap_with_files(self, configmap_name: str, files_data: dict, namespace: str) -> bool:
        """
        Create a ConfigMap with base64-encoded file data.
        
        Args:
            configmap_name: Name of the ConfigMap to create
            files_data: Dictionary where keys are file names and values are base64-encoded content
            namespace: Kubernetes namespace to create the ConfigMap in
            
        Returns:
            True if ConfigMap was created successfully, False otherwise
        """
        try:
            # Create ConfigMap object
            configmap = client.V1ConfigMap(
                api_version="v1",
                kind="ConfigMap",
                metadata=client.V1ObjectMeta(name=configmap_name, namespace=namespace),
                data=files_data
            )
            
            # Create the ConfigMap
            self.core_api.create_namespaced_config_map(namespace=namespace, body=configmap)
            self.logger.info(f"Created ConfigMap {configmap_name} with {len(files_data)} files in namespace {namespace}")
            return True
            
        except client.exceptions.ApiException as e:
            self.logger.error(f"K8s API Error creating ConfigMap {configmap_name}: {e}")
            return False
        except Exception as e:
            self.logger.error(f"Error creating ConfigMap {configmap_name}: {e}")
            return False

    def delete_configmap(self, configmap_name: str, namespace: str) -> bool:
        """
        Delete a ConfigMap.
        
        Args:
            configmap_name: Name of the ConfigMap to delete
            namespace: Kubernetes namespace
            
        Returns:
            True if deletion was successful, False otherwise
        """
        try:
            self.core_api.delete_namespaced_config_map(name=configmap_name, namespace=namespace)
            self.logger.info(f"Deleted ConfigMap {configmap_name} from namespace {namespace}")
            return True
            
        except client.exceptions.ApiException as e:
            if e.status == 404:
                self.logger.info(f"ConfigMap {configmap_name} not found (already deleted)")
                return True
            self.logger.error(f"K8s API Error deleting ConfigMap {configmap_name}: {e}")
            return False
        except Exception as e:
            self.logger.error(f"Error deleting ConfigMap {configmap_name}: {e}")
            return False
