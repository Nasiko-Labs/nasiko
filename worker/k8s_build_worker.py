"""
Kubernetes Build Worker
Listens for orchestration commands from Redis streams and handles agent building/deployment using K8s BuildKit.
This worker is designed to run in a Kubernetes cluster and uses remote BuildKit for image building.
"""

import redis
import json
import logging
import asyncio
import signal
import sys
import aiohttp
import time
import os
from typing import Any
from datetime import datetime, UTC
from pathlib import Path

from app.pkg.config.config import settings
from app.service.k8s_service import K8sService
from app.utils.observability.injector import TracingInjector
from app.utils.observability.config import ObservabilityConfig


class K8sBuildWorker:
    """Kubernetes-native worker for agent building and deployment via BuildKit"""

    def __init__(self, logger: logging.Logger):
        self.logger = logger
        self.redis_client = None
        self.k8s_service = K8sService(logger)
        self.running = False
        self.stream_name = "orchestration:commands"
        self.consumer_group = "k8s-orchestrator"
        # Use pod hostname as unique consumer name (prevents duplicate processing with multiple replicas)
        self.consumer_name = os.environ.get("HOSTNAME", f"k8s-worker-{os.getpid()}")

        # Configuration from settings
        self.registry_url = settings.REGISTRY_URL
        self.gateway_url = settings.GATEWAY_URL  # Public gateway URL for agent access
        self.base_api_url = "http://nasiko-backend.nasiko.svc.cluster.local:8000"
        
        # Initialize observability injector
        # Resolve the installed source directory for app.utils.observability inside the container.
        # The previous relative path (/app/worker/utils/observability) doesn't exist in the image.
        from pathlib import Path
        import app.utils.observability as observability_pkg
        observability_path = str(Path(observability_pkg.__file__).resolve().parent)
        self.tracing_injector = TracingInjector(observability_source_path=observability_path)
        self.observability_config = ObservabilityConfig()

    def connect_redis(self):
        """Connect to Redis server"""
        try:
            self.redis_client = redis.Redis(
                host=settings.REDIS_HOST,
                port=settings.REDIS_PORT,
                db=settings.REDIS_DB,
                decode_responses=True,
                socket_connect_timeout=10,
                socket_timeout=10
            )
            # Test connection
            self.redis_client.ping()
            self.logger.info(f"Connected to Redis at {settings.REDIS_HOST}:{settings.REDIS_PORT}")

            # Create consumer group if it doesn't exist
            try:
                self.redis_client.xgroup_create(
                    self.stream_name,
                    self.consumer_group,
                    id='0',
                    mkstream=True
                )
                self.logger.info(f"Created consumer group '{self.consumer_group}' for stream '{self.stream_name}'")
            except redis.exceptions.ResponseError as e:
                if "BUSYGROUP" in str(e):
                    self.logger.info(f"Consumer group '{self.consumer_group}' already exists")
                else:
                    raise

            return True

        except Exception as e:
            self.logger.error(f"Failed to connect to Redis: {e}")
            return False

    def is_connected(self) -> bool:
        """Check if Redis connection is active"""
        if not self.redis_client:
            return False
        try:
            self.redis_client.ping()
            return True
        except:
            return False

    async def start_listening(self):
        """Start listening for orchestration commands"""
        if not self.connect_redis():
            self.logger.error("Failed to connect to Redis, cannot start listener")
            return

        self.running = True
        self.logger.info("Starting K8s Build Worker for orchestration commands")

        # Handle graceful shutdown
        def signal_handler(signum, frame):
            self.logger.info(f"Received signal {signum}, shutting down gracefully...")
            self.running = False

        signal.signal(signal.SIGINT, signal_handler)
        signal.signal(signal.SIGTERM, signal_handler)

        try:
            while self.running:
                try:
                    # Read messages from stream
                    messages = self.redis_client.xreadgroup(
                        self.consumer_group,
                        self.consumer_name,
                        {self.stream_name: '>'},
                        count=1,
                        block=1000  # Block for 1 second
                    )

                    for stream, msgs in messages:
                        for msg_id, fields in msgs:
                            await self.process_message(msg_id, fields)

                except redis.exceptions.ConnectionError as e:
                    self.logger.error(f"Redis connection error: {e}")
                    if not self.connect_redis():
                        self.logger.error("Failed to reconnect to Redis")
                        break

                except Exception as e:
                    self.logger.error(f"Error reading from Redis stream: {e}")
                    await asyncio.sleep(5)  # Wait before retrying

        except KeyboardInterrupt:
            self.logger.info("Received keyboard interrupt")
        finally:
            self.running = False
            if self.redis_client:
                self.redis_client.close()
            self.logger.info("K8s Build Worker stopped")

    async def process_message(self, msg_id: str, fields: dict[str, str]):
        """Process a single orchestration command message"""
        try:
            self.logger.info(f"Processing message {msg_id}: {fields}")

            # Extract command details
            command = fields.get('command')
            agent_name = fields.get('agent_name')
            agent_path = fields.get('agent_path')
            base_url = fields.get('base_url', self.base_api_url)

            # Extract additional data
            owner_id = fields.get('owner_id')
            upload_id = fields.get('upload_id')
            upload_type = fields.get('upload_type')
            git_url = fields.get('git_url')  # Optional: for git-based builds
            webhook_url = fields.get('webhook_url')  # Optional: for n8n_register uploads
            
            # Extract update-specific data
            action = fields.get('action', command)  # NEW: support action field
            agent_id = fields.get('agent_id')
            new_version = fields.get('new_version')
            previous_version = fields.get('previous_version')
            update_strategy = fields.get('update_strategy', 'rolling')
            cleanup_old = fields.get('cleanup_old', True)
            target_version = fields.get('target_version')  # For rollbacks

            if not all([command, agent_name]):
                self.logger.error(f"Invalid message format: missing required fields")
                await self.acknowledge_message(msg_id)
                return

            # Log extracted ownership information
            if owner_id:
                self.logger.info(f"Processing agent '{agent_name}' for owner: {owner_id} (upload_id: {upload_id}, type: {upload_type})")

            # Set initial status in Redis and update database
            await self.set_agent_status(agent_name, 'processing', {
                'message': 'K8s orchestration command received',
                'stage': 'initializing',
                'owner_id': owner_id,
                'upload_id': upload_id,
                'upload_type': upload_type
            })
            await self.update_database_status(agent_name, base_url, 'orchestration_processing', 95,
                                            'K8s build orchestration started')

            # Process based on action field first, then command for backward compatibility
            if action == 'update_agent':
                await self.handle_update_agent(agent_name, agent_path, base_url, owner_id, upload_id,
                                             upload_type, agent_id, new_version, previous_version,
                                             update_strategy, cleanup_old)
            elif action == 'rollback_agent':
                await self.handle_rollback_agent(agent_name, agent_path, base_url, owner_id,
                                                agent_id, target_version, previous_version)
            elif action == 'rebuild_agent':
                await self.handle_rebuild_agent(agent_name, agent_path, base_url, owner_id,
                                               agent_id, new_version)
            elif command == 'deploy_agent' or action == 'deploy_agent':
                await self.handle_deploy_agent(agent_name, agent_path, base_url, owner_id, upload_id,
                                              upload_type, git_url, webhook_url)
            else:
                self.logger.warning(f"Unknown command/action: {command}/{action}")
                await self.set_agent_status(agent_name, 'error', {
                    'message': f'Unknown command/action: {command}/{action}',
                    'owner_id': owner_id,
                    'upload_id': upload_id
                })

            # Acknowledge message processing
            await self.acknowledge_message(msg_id)

        except Exception as e:
            self.logger.error(f"Error processing message {msg_id}: {e}")
            if 'agent_name' in locals():
                await self.set_agent_status(agent_name, 'error', {
                    'message': f'Failed to process orchestration command: {str(e)}',
                    'owner_id': locals().get('owner_id'),
                    'upload_id': locals().get('upload_id')
                })
            await self.acknowledge_message(msg_id)

    async def handle_deploy_agent(self, agent_name: str, agent_path: str, base_url: str,
                                 owner_id: str | None = None, upload_id: str | None = None,
                                 upload_type: str | None = None, git_url: str | None = None,
                                 webhook_url: str | None = None):
        """Handle agent deployment command using K8s BuildKit"""
        try:
            if owner_id:
                self.logger.info(f"Deploying agent '{agent_name}' via K8s BuildKit for owner: {owner_id}")
            else:
                self.logger.info(f"Deploying agent '{agent_name}' via K8s BuildKit")

            # Generate unique identifiers
            timestamp = int(time.time())
            job_id = f"{agent_name}-{timestamp}"
            build_job_name = f"job-{job_id}"

            # Determine image destination
            image_tag = f"v{timestamp}"
            image_destination = f"{self.registry_url}/{agent_name}:{image_tag}"

            # Set status to building
            await self.set_agent_status(agent_name, 'building', {
                'message': 'Building image with K8s BuildKit',
                'stage': 'buildkit_build',
                'job_id': job_id,
                'image_destination': image_destination,
                'owner_id': owner_id,
                'upload_id': upload_id,
                'upload_type': upload_type
            })
            await self.update_database_status(agent_name, base_url, 'orchestration_processing', 96,
                                            f'Submitting build job {build_job_name}')

            # Extract version from agent_path (format: /app/agents/{name}/v{version})
            version = "1.0.0"  # default fallback
            if agent_path and '/v' in agent_path:
                try:
                    version_with_v = agent_path.split('/v')[-1]
                    # Remove 'v' prefix if present to get semantic version
                    version = version_with_v if not version_with_v.startswith('v') else version_with_v[1:]
                    self.logger.info(f"UPLOAD_VERSION: Extracted version {version} from path {agent_path}")
                except Exception as e:
                    self.logger.warning(f"UPLOAD_VERSION: Failed to extract version from {agent_path}: {e}, using default 1.0.0")
                    version = "1.0.0"

            # Create build record in agent operations collection with version mapping
            build_id = await self.create_build_record_with_version(agent_name, base_url, image_destination, build_job_name, version)
            self.logger.info(f"Created build record for agent '{agent_name}' with build ID {build_id} and version {version}")
            
            # Step 0: Inject Observability (if enabled) and get modified files path
            modified_files_path = await self._inject_observability_if_enabled(agent_name, base_url, agent_path)
            
            # Step 1: Create Build Job
            # Use git_url if provided, otherwise use uploaded files (or modified files if observability injection succeeded)
            if git_url:
                self.logger.info(f"Creating BuildKit job for {agent_name} from git: {git_url}")
                build_success = self.k8s_service.create_build_job(
                    job_id=job_id,
                    git_url=git_url,
                    image_destination=image_destination
                )
            else:
                # Build from uploaded files (zip/directory uploads)
                self.logger.info(f"Creating BuildKit job for {agent_name} from uploaded files")
                build_success = self.k8s_service.create_build_job_from_upload(
                    job_id=job_id,
                    agent_name=agent_name,
                    image_destination=image_destination,
                    backend_url=base_url,
                    agent_path=agent_path,
                    local_files_path=modified_files_path
                )

            if not build_success:
                raise Exception("Failed to create K8s build job")

            self.logger.info(f"Build job {build_job_name} submitted, monitoring status...")
            await self.update_database_status(agent_name, base_url, 'orchestration_processing', 97,
                                            f'Build job submitted, waiting for completion')

            # Step 2: Poll Job Status
            max_wait_time = 600  # 10 minutes max
            poll_interval = 5  # seconds
            elapsed_time = 0

            while elapsed_time < max_wait_time:
                job_status = self.k8s_service.get_job_status(build_job_name)
                self.logger.debug(f"Build job {build_job_name} status: {job_status}")

                if job_status == "succeeded":
                    self.logger.info(f"Build job {build_job_name} succeeded!")
                    # Update build status to completed
                    if build_id:
                        await self.update_build_status(build_id, base_url, "success", agent_id=agent_name)
                    break
                elif job_status == "failed":
                    # Update build status to failed
                    if build_id:
                        await self.update_build_status(build_id, base_url, "failed", error_message=f"Build job {build_job_name} failed", agent_id=agent_name)
                    raise Exception(f"Build job {build_job_name} failed")
                elif job_status in ["active", "pending"]:
                    # Still running, wait and check again
                    await asyncio.sleep(poll_interval)
                    elapsed_time += poll_interval
                else:
                    self.logger.warning(f"Unknown job status: {job_status}")
                    await asyncio.sleep(poll_interval)
                    elapsed_time += poll_interval

            if elapsed_time >= max_wait_time:
                raise Exception(f"Build job {build_job_name} timed out after {max_wait_time} seconds")

            # Step 3: Deploy Agent
            self.logger.info(f"Deploying agent {agent_name} with image {image_destination}")
            await self.set_agent_status(agent_name, 'deploying', {
                'message': 'Deploying agent to cluster',
                'stage': 'k8s_deployment',
                'image': image_destination
            })
            await self.update_database_status(agent_name, base_url, 'orchestration_processing', 98,
                                            'Image built, deploying to cluster')

            deployment_name = f"agent-{agent_name}-{timestamp}"
            
            # Create deployment record in agent operations collection
            deployment_id = await self.create_deployment_record(agent_name, base_url, build_id, deployment_name)
            
            # Prepare environment variables
            env_vars = {
                'AGENT_NAME': agent_name,
                'OWNER_ID': owner_id or '',
                'OPENAI_API_KEY': settings.OPENAI_API_KEY
            }
            
            # Add observability environment variables
            obs_env_vars = await self.get_observability_env_vars(agent_name)
            env_vars.update(obs_env_vars)
            
            # Add WEBHOOK_URL for n8n agents
            if upload_type == "n8n_register" and webhook_url:
                env_vars['WEBHOOK_URL'] = webhook_url
            
            deploy_result = self.k8s_service.deploy_agent(
                deployment_name=deployment_name,
                image_reference=image_destination,
                port=5000,
                env_vars=env_vars
            )

            if not deploy_result:
                # Update deployment status to failed
                if deployment_id:
                    await self.update_deployment_status(deployment_id, base_url, "failed", error_message="Failed to deploy agent to K8s", agent_id=agent_name)
                raise Exception("Failed to deploy agent to K8s")

            # Construct public agent URL using gateway
            # Format: http://<gateway-ip>/agents/<deployment-name>
            # Use deployment_name (with timestamp) to match Kong route
            if not self.gateway_url:
                raise Exception("GATEWAY_URL not configured. Cannot register agent without public gateway URL.")

            # Ensure gateway_url doesn't have trailing slash
            gateway_base = self.gateway_url.rstrip('/')
            if self.gateway_url == "http://localhost":
                gateway_base = gateway_base + ':8000' # for local deployment
            agent_url = f"{gateway_base}/agents/{deployment_name}"

            self.logger.info(f"Agent will be accessible at: {agent_url}")

            # Step 4: Register Agent in Registry
            self.logger.info(f"Registering agent {agent_name} in registry")
            registry_updated = await self.register_agent_in_registry(
                agent_name=agent_name,
                service_url=agent_url,
                owner_id=owner_id,
                base_url=base_url,
                agent_path=agent_path
            )

            # Step 5: Create Agent Permissions (if registry was updated and owner_id is provided)
            permissions_created = False
            if registry_updated and owner_id:
                self.logger.info(f"Creating permissions for agent {agent_name} with owner {owner_id}")
                permissions_created = await self.create_agent_permissions(agent_name, owner_id)
                if not permissions_created:
                    self.logger.warning(f"Registry updated but permission creation failed for agent {agent_name}")
            elif registry_updated and not owner_id:
                self.logger.info(f"Registry updated for agent {agent_name} but no owner_id provided, skipping permissions")

            # Update deployment status to running
            if deployment_id:
                await self.update_deployment_status(deployment_id, base_url, "running", service_url=agent_url, agent_id=agent_name)

            # Success!
            await self.set_agent_status(agent_name, 'running', {
                'message': 'Agent deployed successfully via K8s',
                'stage': 'deployed',
                'url': agent_url,
                'deployment_name': deployment_name,
                'image': image_destination
            })

            await self.update_database_status(agent_name, base_url, 'completed', 100,
                                            'Agent built and deployed successfully', {
                                                'url': agent_url,
                                                'registry_updated': registry_updated,
                                                'permissions_created': permissions_created,
                                                'image': image_destination,
                                                'deployment_name': deployment_name
                                            })
            self.logger.info(f"Successfully deployed agent '{agent_name}' to {agent_url}")
            
            # Update registry version status to 'active' after successful deployment
            await self._update_registry_version_status(agent_name, "active", base_url)

        except Exception as e:
            self.logger.error(f"Error deploying agent '{agent_name}': {e}")
            await self.set_agent_status(agent_name, 'failed', {
                'message': f'Deployment failed: {str(e)}',
                'stage': 'deployment_error'
            })
            await self.update_database_status(agent_name, base_url, 'failed', 0,
                                            f"K8s deployment failed: {str(e)}", {
                                                'error_details': [str(e)]
                                            })

    async def fetch_agentcard_from_backend(self, agent_name: str, base_url: str, version: str | None = None) -> dict[str, Any] | None:
        """
        Download agent tarball from backend and extract AgentCard.json.
        Falls back to generating AgentCard if not found.
        """
        import tarfile
        import tempfile

        tar_path = None
        try:
            # Download agent tarball from backend (use versioned endpoint only for proper versions)
            # Skip version for N8N agents and other non-versioned agents
            if version and (version.replace('.', '').replace('-', '').isdigit() or version.split('.')[0].isdigit()):
                download_url = f"{base_url}/api/v1/agents/{agent_name}/download?version={version}"
                self.logger.info(f"Downloading versioned agent files for version {version} from {download_url}")
            else:
                download_url = f"{base_url}/api/v1/agents/{agent_name}/download"
                self.logger.info(f"Downloading agent files from {download_url}")

            async with aiohttp.ClientSession() as session:
                async with session.get(download_url, timeout=aiohttp.ClientTimeout(total=30)) as response:
                    if response.status != 200:
                        self.logger.warning(f"Failed to download agent files: HTTP {response.status}")
                        return None

                    # Save tarball to temp file
                    with tempfile.NamedTemporaryFile(mode='wb', suffix='.tar.gz', delete=False) as tmp_tar:
                        tmp_tar.write(await response.read())
                        tar_path = tmp_tar.name

            # Extract tarball to temp directory
            with tempfile.TemporaryDirectory() as extract_dir:
                self.logger.info(f"Extracting agent files to {extract_dir}")
                with tarfile.open(tar_path, 'r:gz') as tar:
                    tar.extractall(extract_dir)

                # Look for AgentCard.json
                agentcard_path = Path(extract_dir) / "AgentCard.json"

                if agentcard_path.exists():
                    self.logger.info(f"Found AgentCard.json for {agent_name}")
                    with open(agentcard_path, 'r') as f:
                        return json.load(f)
                else:
                    self.logger.warning(f"AgentCard.json not found in agent files, attempting to generate")
                    return await self.generate_agentcard(extract_dir, agent_name)

        except Exception as e:
            self.logger.error(f"Error fetching/generating AgentCard for {agent_name}: {e}")
            return None
        finally:
            # Always cleanup temp tar file
            if tar_path:
                Path(tar_path).unlink(missing_ok=True)
    
    async def generate_agentcard(self, agent_path: str, agent_name: str) -> dict[str, Any] | None:
        """Generate AgentCard using the AgentCard Generator"""
        try:
            from app.utils.agentcard_generator import AgentCardGeneratorAgent
            
            self.logger.info(f"Generating AgentCard for {agent_name} using AgentCard Generator")

            # Check if OPENAI_API_KEY is available
            openai_key = getattr(settings, 'OPENAI_API_KEY', None)
            if not openai_key:
                self.logger.warning("OPENAI_API_KEY not configured, cannot generate AgentCard")
                return None
            
            # Initialize generator
            generator = AgentCardGeneratorAgent(api_key=openai_key)
            
            # Generate AgentCard (this is a sync method, run in thread pool)
            result = await asyncio.to_thread(
                generator.generate_agentcard,
                agent_path=agent_path,
                verbose=False
            )
            
            if result.get("status") == "success" and result.get("agentcard"):
                self.logger.info(f"Successfully generated AgentCard for {agent_name} with {result.get('iterations', 0)} iterations")
                return result["agentcard"]
            else:
                self.logger.warning(f"AgentCard generation failed: {result.get('message', 'Unknown error')}")
                return None
                
        except ImportError as e:
            self.logger.error(f"AgentCard Generator not available: {e}")
            return None
        except Exception as e:
            self.logger.error(f"Error generating AgentCard: {e}")
            return None

    async def register_agent_in_registry(
        self,
        agent_name: str,
        service_url: str,
        owner_id: str | None,
        base_url: str,
        agent_path: str | None = None,
    ) -> bool:
        """Register or update agent in the registry via API"""
        try:
            # Try to fetch AgentCard from backend or generate it
            # Extract version from agent_path if available for versioned download
            version = None
            if agent_path and '/v' in agent_path:
                version = agent_path.split('/v')[-1]
            
            agentcard_data = await self.fetch_agentcard_from_backend(agent_name, base_url, version)
            
            if agentcard_data:
                self.logger.info(f"Using AgentCard data for {agent_name} with {len(agentcard_data.get('skills', []))} skills")

                # Use full AgentCard data (deep copy to avoid modifying original)
                registry_data = json.loads(json.dumps(agentcard_data))
                
                # Override/ensure critical K8s-specific fields
                registry_data["id"] = agent_name
                registry_data["url"] = service_url
                registry_data["deployment_type"] = "kubernetes"
                
                if owner_id:
                    registry_data["owner_id"] = owner_id
            else:
                # Fallback to minimal registry entry
                self.logger.warning(f"No AgentCard found/generated for {agent_name}, using minimal capabilities")
                registry_data = {
                    "id": agent_name,
                    "name": agent_name,
                    "url": service_url,
                    "description": f"Agent deployed via K8s BuildKit",
                    "capabilities": {
                        "tools": [],
                        "prompts": []
                    },
                    "version": "1.0.0",
                    "deployment_type": "kubernetes"
                }
                
                if owner_id:
                    registry_data["owner_id"] = owner_id

            # Call registry API
            url = f"{base_url}/api/v1/registry/agent/{agent_name}"

            # Log the data being sent for debugging
            self.logger.info(f"Registering agent {agent_name} with data: {json.dumps(registry_data, indent=2)[:500]}...")

            async with aiohttp.ClientSession() as session:
                async with session.put(url, json=registry_data, timeout=aiohttp.ClientTimeout(total=10)) as response:
                    if response.status in [200, 201]:
                        self.logger.info(f"Registered agent {agent_name} in registry")
                        return True
                    else:
                        # Get detailed error response
                        try:
                            error_detail = await response.text()
                            self.logger.warning(f"Failed to register agent {agent_name}: {response.status} - {error_detail}")
                        except:
                            self.logger.warning(f"Failed to register agent {agent_name}: {response.status}")
                        return False

        except Exception as e:
            self.logger.error(f"Error registering agent {agent_name}: {e}")
            return False

    async def create_agent_permissions(self, agent_id: str, owner_id: str) -> bool:
        """Create agent permissions in the auth service"""
        try:
            # Use auth service URL (same pattern as orchestrator)
            auth_service_url = os.environ.get("AUTH_SERVICE_URL")
            url = f"{auth_service_url}/auth/agents/{agent_id}/permissions"
            
            self.logger.info(f"Creating permissions for agent {agent_id} with owner {owner_id}")
            
            async with aiohttp.ClientSession() as session:
                async with session.post(url, params={"owner_id": owner_id}, 
                                      timeout=aiohttp.ClientTimeout(total=30)) as response:
                    if response.status in [200, 201]:
                        self.logger.info(f"Successfully created permissions for agent {agent_id}")
                        return True
                    else:
                        try:
                            error_detail = await response.text()
                            self.logger.error(f"Failed to create permissions for agent {agent_id}: {response.status} - {error_detail}")
                        except:
                            self.logger.error(f"Failed to create permissions for agent {agent_id}: {response.status}")
                        return False
                        
        except asyncio.TimeoutError:
            self.logger.error(f"Timeout creating permissions for agent {agent_id}")
            return False
        except Exception as e:
            self.logger.error(f"Error creating permissions for agent {agent_id}: {e}")
            return False

    async def set_agent_status(self, agent_name: str, status: str, details: dict[str, Any] | None = None):
        """Set agent deployment status in Redis"""
        if not self.is_connected():
            return

        try:
            status_key = f"agent:status:{agent_name}"
            status_data = {
                "agent_name": agent_name,
                "status": status,
                "last_updated": datetime.now(UTC).isoformat(),
                "updated_by": "k8s-worker"
            }

            if details:
                # Filter out None values that Redis can't store
                filtered_details = {k: v for k, v in details.items() if v is not None}
                status_data.update(filtered_details)

            # Store as hash for easy field access
            self.redis_client.hset(status_key, mapping=status_data)

            # Set expiration (24 hours)
            self.redis_client.expire(status_key, 86400)

            self.logger.debug(f"Set agent status for {agent_name}: {status}")

        except Exception as e:
            self.logger.error(f"Failed to set agent status for {agent_name}: {e}")

    async def update_database_status(
        self,
        agent_name: str,
        base_url: str,
        status: str,
        progress: int,
        message: str,
        additional_data: dict[str, Any] | None = None,
    ):
        """Update upload status in database via API call to the backend"""
        try:
            update_data = {
                "status": status,
                "progress_percentage": progress,
                "status_message": message,
                "orchestration_duration": None
            }

            if additional_data:
                update_data.update(additional_data)

            # Make API call to update status
            url = f"{base_url}/api/v1/upload-status/agent/{agent_name}/latest"

            async with aiohttp.ClientSession() as session:
                async with session.put(url, json=update_data, timeout=aiohttp.ClientTimeout(total=10)) as response:
                    if response.status == 200:
                        self.logger.debug(f"Updated database status for {agent_name}: {status}")
                    else:
                        self.logger.warning(f"Failed to update database status for {agent_name}: {response.status}")

        except asyncio.TimeoutError:
            self.logger.warning(f"Timeout updating database status for {agent_name}")
        except Exception as e:
            self.logger.error(f"Error updating database status for {agent_name}: {e}")

    async def create_build_record(self, agent_name: str, base_url: str, image_reference: str, k8s_job_name: str) -> str | None:
        """Create build record in agent operations collection"""
        try:
            url = f"{base_url}/api/v1/agents/build"
            
            build_data = {
                "agent_id": agent_name,
                "github_url": None,  # Will be set if we have git_url
                "version_tag": "latest",
                "image_reference": image_reference,
                "status": "building",
                "k8s_job_name": k8s_job_name
            }
            
            async with aiohttp.ClientSession() as session:
                async with session.post(url, json=build_data, timeout=aiohttp.ClientTimeout(total=10)) as response:
                    if response.status == 201:
                        result = await response.json()
                        build_id = result.get("_id")
                        self.logger.info(f"Created build record for {agent_name}: {build_id}")
                        return build_id
                    else:
                        self.logger.warning(f"Failed to create build record for {agent_name}: {response.status}")
                        return None
                        
        except asyncio.TimeoutError:
            self.logger.warning(f"Timeout creating build record for {agent_name}")
            return None
        except Exception as e:
            self.logger.error(f"Error creating build record for {agent_name}: {e}")
            return None

    async def update_build_status(
        self,
        build_id: str,
        base_url: str,
        status: str,
        logs: str | None = None,
        error_message: str | None = None,
        agent_id: str | None = None,
    ):
        """Update build record status"""
        try:
            url = f"{base_url}/api/v1/agents/build/{build_id}/status"
            
            update_data = {
                "agent_id": agent_id or "",  # Provide actual agent_id if available
                "status": status
            }
            
            if logs:
                update_data["logs"] = logs
            if error_message:
                update_data["error_message"] = error_message
            
            async with aiohttp.ClientSession() as session:
                async with session.put(url, json=update_data, timeout=aiohttp.ClientTimeout(total=10)) as response:
                    if response.status == 200:
                        self.logger.debug(f"Updated build status for {build_id}: {status}")
                    else:
                        self.logger.warning(f"Failed to update build status for {build_id}: {response.status}")
                        
        except asyncio.TimeoutError:
            self.logger.warning(f"Timeout updating build status for {build_id}")
        except Exception as e:
            self.logger.error(f"Error updating build status for {build_id}: {e}")

    async def create_deployment_record(self, agent_name: str, base_url: str, build_id: str | None, k8s_deployment_name: str) -> str | None:
        """Create deployment record in agent operations collection"""
        try:
            url = f"{base_url}/api/v1/agents/deploy"
            
            deploy_data = {
                "agent_id": agent_name,
                "build_id": build_id,
                "status": "starting",
                "k8s_deployment_name": k8s_deployment_name,
                "namespace": "nasiko-agents"
            }
            
            async with aiohttp.ClientSession() as session:
                async with session.post(url, json=deploy_data, timeout=aiohttp.ClientTimeout(total=10)) as response:
                    if response.status == 201:
                        result = await response.json()
                        deployment_id = result.get("_id")
                        self.logger.info(f"Created deployment record for {agent_name}: {deployment_id}")
                        return deployment_id
                    else:
                        self.logger.warning(f"Failed to create deployment record for {agent_name}: {response.status}")
                        return None
                        
        except asyncio.TimeoutError:
            self.logger.warning(f"Timeout creating deployment record for {agent_name}")
            return None
        except Exception as e:
            self.logger.error(f"Error creating deployment record for {agent_name}: {e}")
            return None

    async def update_deployment_status(
        self,
        deployment_id: str,
        base_url: str,
        status: str,
        service_url: str | None = None,
        error_message: str | None = None,
        agent_id: str | None = None,
    ):
        """Update deployment record status"""
        try:
            url = f"{base_url}/api/v1/agents/deployment/{deployment_id}/status"
            
            update_data = {
                "agent_id": agent_id or "",  # Provide actual agent_id if available
                "status": status
            }
            
            if service_url:
                update_data["service_url"] = service_url
            if error_message:
                update_data["error_message"] = error_message
            
            async with aiohttp.ClientSession() as session:
                async with session.put(url, json=update_data, timeout=aiohttp.ClientTimeout(total=10)) as response:
                    if response.status == 200:
                        self.logger.debug(f"Updated deployment status for {deployment_id}: {status}")
                    else:
                        self.logger.warning(f"Failed to update deployment status for {deployment_id}: {response.status}")
                        
        except asyncio.TimeoutError:
            self.logger.warning(f"Timeout updating deployment status for {deployment_id}")
        except Exception as e:
            self.logger.error(f"Error updating deployment status for {deployment_id}: {e}")

    async def handle_update_agent(
        self,
        agent_name: str,
        agent_path: str | None,
        base_url: str,
        owner_id: str | None,
        upload_id: str | None,
        upload_type: str | None,
        agent_id: str | None,
        new_version: str | None,
        previous_version: str | None,
        update_strategy: str,
        cleanup_old: bool,
    ):
        """Handle agent update command with version management and cleanup"""
        try:
            self.logger.info(f"AGENT_UPDATE: Updating agent '{agent_name}' from {previous_version} to {new_version}")
            
            # Initial status updates (like handle_deploy_agent)
            await self.set_agent_status(agent_name, 'updating', {
                'message': f'Updating agent from {previous_version} to {new_version}',
                'stage': 'update_initializing',
                'new_version': new_version,
                'previous_version': previous_version,
                'owner_id': owner_id,
                'upload_id': upload_id,
                'upload_type': upload_type
            })
            await self.update_database_status(agent_name, base_url, 'orchestration_processing', 95,
                                            f'K8s update orchestration started: {previous_version} → {new_version}')
            
            # Generate versioned image tag
            timestamp = int(time.time())
            job_id = f"{agent_name}-{timestamp}"  # Simplified job_id like deploy_agent
            build_job_name = f"job-{job_id}"
            image_tag = f"v{timestamp}"  # Use timestamp like deploy_agent for uniqueness
            image_destination = f"{self.registry_url}/{agent_name}:{image_tag}"
            
            # Step 1: Build new version
            await self.set_agent_status(agent_name, 'building', {
                'message': f'Building updated image with K8s BuildKit (v{new_version})',
                'stage': 'buildkit_build',
                'job_id': job_id,
                'image_destination': image_destination,
                'owner_id': owner_id,
                'upload_id': upload_id,
                'upload_type': upload_type
            })
            await self.update_database_status(agent_name, base_url, 'orchestration_processing', 96,
                                            f'Submitting build job {build_job_name} for version {new_version}')
            
            # Create build record with version info
            build_id = await self.create_build_record_with_version(
                agent_name, base_url, image_destination, build_job_name, new_version
            )
            if build_id:
                self.logger.info(f"Created build record for agent '{agent_name}' update with build ID {build_id}")
            
            # Step 0: Inject Observability (if enabled) and get modified files path
            modified_files_path = await self._inject_observability_if_enabled(agent_name, base_url, agent_path)
            
            # Build new version using modified files if observability injection succeeded, otherwise use original
            build_success = self.k8s_service.create_build_job_from_upload(
                job_id=job_id,
                agent_name=agent_name,
                image_destination=image_destination,
                backend_url=base_url,
                agent_path=agent_path,  # Original agent path for backend downloads
                local_files_path=modified_files_path  # Set to agent_name if observability injection succeeded
            )
            
            if not build_success:
                raise Exception("Failed to create K8s update build job")
            
            self.logger.info(f"Update build job {build_job_name} submitted, monitoring status...")
            await self.update_database_status(agent_name, base_url, 'orchestration_processing', 97,
                                            f'Build job submitted for update, waiting for completion')
            
            # Wait for build completion
            await self._wait_for_build_completion(build_job_name, build_id, base_url, agent_name)
            
            # Step 2: Deploy new version
            self.logger.info(f"Deploying updated agent {agent_name} with image {image_destination}")
            await self.set_agent_status(agent_name, 'deploying', {
                'message': f'Deploying updated agent to cluster (v{new_version})',
                'stage': 'k8s_deployment',
                'image': image_destination,
                'update_strategy': update_strategy
            })
            await self.update_database_status(agent_name, base_url, 'orchestration_processing', 98,
                                            f'Image built, deploying updated agent using {update_strategy} strategy')
            
            await self._deploy_updated_version(
                agent_name, image_destination, new_version, timestamp, base_url,
                owner_id, upload_type, build_id, update_strategy, agent_path
            )
            
            # Step 3: Cleanup old deployments if requested
            if cleanup_old and previous_version:
                self.logger.info(f"Cleaning up old deployments for {agent_name} version {previous_version}")
                await self._cleanup_old_agent_deployments(agent_id, previous_version)
            
            # Step 4: Finalize update and update registry version status
            await self._finalize_agent_update(agent_id, new_version, previous_version, base_url)
            
            # Update registry version status to 'active' after successful deployment (like handle_deploy_agent)
            await self._update_registry_version_status(agent_name, "active", base_url)
            
            # Final success status
            await self.set_agent_status(agent_name, 'updated', {
                'message': f'Agent successfully updated to version {new_version}',
                'stage': 'update_completed',
                'active_version': new_version,
                'previous_version': previous_version,
                'image': image_destination
            })
            
            await self.update_database_status(agent_name, base_url, 'completed', 100,
                                            f'Agent successfully updated: {previous_version} → {new_version}', {
                                                'update_strategy': update_strategy,
                                                'image': image_destination,
                                                'active_version': new_version,
                                                'cleanup_performed': cleanup_old
                                            })
            
            self.logger.info(f"AGENT_UPDATE: Successfully updated {agent_name} from {previous_version} to {new_version}")
            
        except Exception as e:
            self.logger.error(f"AGENT_UPDATE: Update failed for {agent_name}: {e}")
            await self.set_agent_status(agent_name, 'update_failed', {
                'message': f'Update failed: {str(e)}',
                'stage': 'update_error',
                'new_version': new_version,
                'previous_version': previous_version
            })
            await self.update_database_status(agent_name, base_url, 'failed', 0,
                                            f"K8s agent update failed: {str(e)}", {
                                                'error_details': [str(e)],
                                                'failed_version': new_version,
                                                'previous_version': previous_version
                                            })
            raise

    async def handle_rollback_agent(
        self,
        agent_name: str,
        agent_path: str,
        base_url: str,
        owner_id: str | None,
        agent_id: str | None,
        target_version: str | None,
        current_version: str | None,
    ):
        """Handle agent rollback to previous version"""
        try:
            self.logger.info(f"AGENT_ROLLBACK: Rolling back agent '{agent_name}' from {current_version} to {target_version}")
            
            await self.set_agent_status(agent_name, 'rolling_back', {
                'message': f'Rolling back from {current_version} to {target_version}',
                'stage': 'rollback_start',
                'target_version': target_version,
                'current_version': current_version
            })
            
            # Step 1: Use existing versioned files for target version
            versioned_path = f"{agent_path}/v{target_version}" if 'v' not in agent_path else agent_path
            
            # Step 2: Deploy target version (reuse deployment logic)
            timestamp = int(time.time())
            deployment_name = f"agent-{agent_name}-{timestamp}"
            
            # Resolve the actual image tag for the target semantic version
            resolved_image_tag = await self._resolve_version_to_image_tag(agent_name, target_version, base_url)
            image_destination = f"{self.registry_url}/{agent_name}:{resolved_image_tag}"
            
            self.logger.info(f"ROLLBACK: Resolved version {target_version} to image tag {resolved_image_tag}")
            
            # Create deployment record for rollback
            deployment_id = await self.create_deployment_record(agent_name, base_url, None, deployment_name)
            
            env_vars = {
                'AGENT_NAME': agent_name,
                'OWNER_ID': owner_id or '',
                'OPENAI_API_KEY': settings.OPENAI_API_KEY
            }
            
            # Add observability environment variables
            obs_env_vars = await self.get_observability_env_vars(agent_name)
            env_vars.update(obs_env_vars)
            
            deploy_result = self.k8s_service.deploy_agent(
                deployment_name=deployment_name,
                image_reference=image_destination,
                port=5000,
                env_vars=env_vars
            )
            
            if not deploy_result:
                raise Exception("Failed to deploy rollback version")
            
            # Step 3: Update service URLs and cleanup failed version
            gateway_base = self.gateway_url.rstrip('/')
            agent_url = f"{gateway_base}/agents/{deployment_name}"
            
            # Update registry back to target version
            await self.register_agent_in_registry(agent_name, agent_url, owner_id, base_url, versioned_path)
            
            if deployment_id:
                await self.update_deployment_status(deployment_id, base_url, "RUNNING", service_url=agent_url, agent_id=agent_name)
            
            # Step 4: Cleanup current failed deployment
            if current_version:
                await self._cleanup_old_agent_deployments(agent_id, current_version)
            
            await self.set_agent_status(agent_name, 'rolled_back', {
                'message': f'Successfully rolled back to {target_version}',
                'stage': 'rollback_complete',
                'url': agent_url,
                'active_version': target_version
            })
            
            self.logger.info(f"AGENT_ROLLBACK: Successfully rolled back {agent_name} to {target_version}")
            
        except Exception as e:
            self.logger.error(f"AGENT_ROLLBACK: Rollback failed for {agent_name}: {e}")
            await self.set_agent_status(agent_name, 'rollback_failed', {
                'message': f'Rollback failed: {str(e)}',
                'stage': 'rollback_error'
            })
            raise

    async def handle_rebuild_agent(
        self,
        agent_name: str,
        agent_path: str,
        base_url: str,
        owner_id: str | None,
        agent_id: str | None,
        version: str | None,
    ):
        """Handle agent rebuild with same code (e.g., for base image updates)"""
        try:
            self.logger.info(f"AGENT_REBUILD: Rebuilding agent '{agent_name}' version {version}")
            
            await self.set_agent_status(agent_name, 'rebuilding', {
                'message': f'Rebuilding version {version}',
                'stage': 'rebuild_start',
                'version': version
            })
            
            # Step 1: Rebuild with new timestamp but same version
            timestamp = int(time.time())
            job_id = f"{agent_name}-rebuild-{timestamp}"
            build_job_name = f"job-{job_id}"
            image_tag = f"v{version}-rebuild-{timestamp}"
            image_destination = f"{self.registry_url}/{agent_name}:{image_tag}"
            
            # Create build record
            build_id = await self.create_build_record_with_version(
                agent_name, base_url, image_destination, build_job_name, version
            )
            
            # Step 0: Inject Observability (if enabled) and get modified files path
            modified_files_path = await self._inject_observability_if_enabled(agent_name, base_url, agent_path)
            
            # Rebuild using existing files (with observability if injection succeeded)
            build_success = self.k8s_service.create_build_job_from_upload(
                job_id=job_id,
                agent_name=agent_name,
                image_destination=image_destination,
                backend_url=base_url,
                agent_path=agent_path,
                local_files_path=modified_files_path
            )
            
            if not build_success:
                raise Exception("Failed to create rebuild job")
            
            # Wait for build completion
            await self._wait_for_build_completion(build_job_name, build_id, base_url, agent_name)
            
            # Step 2: Deploy rebuilt version (replace existing)
            deployment_name = f"agent-{agent_name}-{timestamp}"
            deployment_id = await self.create_deployment_record(agent_name, base_url, build_id, deployment_name)
            
            env_vars = {
                'AGENT_NAME': agent_name,
                'OWNER_ID': owner_id or '',
                'OPENAI_API_KEY': settings.OPENAI_API_KEY
            }
            
            # Add observability environment variables
            obs_env_vars = await self.get_observability_env_vars(agent_name)
            env_vars.update(obs_env_vars)
            
            deploy_result = self.k8s_service.deploy_agent(
                deployment_name=deployment_name,
                image_reference=image_destination,
                port=5000,
                env_vars=env_vars
            )
            
            if not deploy_result:
                raise Exception("Failed to deploy rebuilt agent")
            
            # Step 3: Update registry and cleanup old deployment
            gateway_base = self.gateway_url.rstrip('/')
            agent_url = f"{gateway_base}/agents/{deployment_name}"
            
            await self.register_agent_in_registry(agent_name, agent_url, owner_id, base_url, agent_path)
            
            if deployment_id:
                await self.update_deployment_status(deployment_id, base_url, "RUNNING", service_url=agent_url, agent_id=agent_name)
            
            # Cleanup previous deployment of same version
            await self._cleanup_old_agent_deployments(agent_id, version, keep_latest=1)
            
            await self.set_agent_status(agent_name, 'rebuilt', {
                'message': f'Successfully rebuilt version {version}',
                'stage': 'rebuild_complete',
                'url': agent_url,
                'image': image_destination
            })
            
            self.logger.info(f"AGENT_REBUILD: Successfully rebuilt {agent_name} version {version}")
            
        except Exception as e:
            self.logger.error(f"AGENT_REBUILD: Rebuild failed for {agent_name}: {e}")
            await self.set_agent_status(agent_name, 'rebuild_failed', {
                'message': f'Rebuild failed: {str(e)}',
                'stage': 'rebuild_error'
            })
            raise

    async def create_build_record_with_version(
        self,
        agent_name: str,
        base_url: str,
        image_reference: str,
        k8s_job_name: str,
        version: str,
    ) -> str | None:
        """Create build record with version information"""
        try:
            url = f"{base_url}/api/v1/agents/build"
            
            # Extract timestamp from image_reference for version mapping
            # image_reference format: "{registry_url}/{agent_name}:v{timestamp}"
            timestamp = None
            if ":v" in image_reference:
                timestamp = image_reference.split(":v")[-1]
            
            build_data = {
                "agent_id": agent_name,
                "version_tag": version,  # Semantic version (e.g., "2.0.0")
                "image_reference": image_reference,  # Timestamp-based tag (e.g., "myagent:v1736686234")
                "status": "building",
                "k8s_job_name": k8s_job_name,
                "version_mapping": {
                    "semantic_version": version,
                    "image_tag": f"v{timestamp}" if timestamp else None,
                    "timestamp": int(timestamp) if timestamp else int(time.time())
                }
            }
            
            async with aiohttp.ClientSession() as session:
                async with session.post(url, json=build_data, timeout=aiohttp.ClientTimeout(total=10)) as response:
                    if response.status == 201:
                        result = await response.json()
                        build_id = result.get("_id")
                        self.logger.info(f"Created versioned build record for {agent_name} v{version}: {build_id}")
                        return build_id
                    else:
                        self.logger.warning(f"Failed to create build record for {agent_name}: {response.status}")
                        return None
                        
        except Exception as e:
            self.logger.error(f"Error creating versioned build record for {agent_name}: {e}")
            return None

    async def _wait_for_build_completion(
        self,
        build_job_name: str,
        build_id: str | None,
        base_url: str,
        agent_name: str | None = None,
    ):
        """Wait for build job to complete and update status"""
        max_wait_time = 600  # 10 minutes
        poll_interval = 5  # seconds
        elapsed_time = 0
        
        while elapsed_time < max_wait_time:
            job_status = self.k8s_service.get_job_status(build_job_name)
            self.logger.debug(f"Build job {build_job_name} status: {job_status}")
            
            if job_status == "succeeded":
                self.logger.info(f"Build job {build_job_name} succeeded!")
                if build_id:
                    await self.update_build_status(build_id, base_url, "success", agent_id=agent_name)
                break
            elif job_status == "failed":
                if build_id:
                    await self.update_build_status(build_id, base_url, "failed", error_message=f"Build job {build_job_name} failed", agent_id=agent_name)
                raise Exception(f"Build job {build_job_name} failed")
            elif job_status in ["active", "pending"]:
                await asyncio.sleep(poll_interval)
                elapsed_time += poll_interval
            else:
                self.logger.warning(f"Unknown job status: {job_status}")
                await asyncio.sleep(poll_interval)
                elapsed_time += poll_interval
        
        if elapsed_time >= max_wait_time:
            raise Exception(f"Build job {build_job_name} timed out after {max_wait_time} seconds")

    async def _deploy_updated_version(
        self,
        agent_name: str,
        image_destination: str,
        new_version: str,
        timestamp: int,
        base_url: str,
        owner_id: str | None,
        upload_type: str | None,
        build_id: str | None,
        update_strategy: str,
        agent_path: str | None = None,
    ):
        """Deploy updated version using specified strategy"""
        deployment_name = f"agent-{agent_name}-{timestamp}"
        
        # Create deployment record
        deployment_id = await self.create_deployment_record(agent_name, base_url, build_id, deployment_name)
        
        env_vars = {
            'AGENT_NAME': agent_name,
            'OWNER_ID': owner_id or '',
            'OPENAI_API_KEY': settings.OPENAI_API_KEY
        }
        
        # Add observability environment variables
        obs_env_vars = await self.get_observability_env_vars(agent_name)
        env_vars.update(obs_env_vars)
        
        if upload_type == "n8n_register":
            env_vars['WEBHOOK_URL'] = f"http://webhook-placeholder/{agent_name}"  # TODO: Get actual webhook
        
        # Deploy based on strategy
        if update_strategy == "blue-green":
            # For blue-green, we deploy alongside existing, then switch
            # This is simplified - full blue-green would require more sophisticated traffic switching
            self.logger.info(f"Deploying {agent_name} using blue-green strategy")
        else:
            # Rolling update - K8s handles this naturally when we update the deployment
            self.logger.info(f"Deploying {agent_name} using rolling update strategy")
        
        deploy_result = self.k8s_service.deploy_agent(
            deployment_name=deployment_name,
            image_reference=image_destination,
            port=5000,
            env_vars=env_vars
        )
        
        if not deploy_result:
            if deployment_id:
                await self.update_deployment_status(deployment_id, base_url, "failed", error_message="Failed to deploy updated agent", agent_id=agent_name)
            raise Exception("Failed to deploy updated agent to K8s")
        
        # Update agent URL in registry
        gateway_base = self.gateway_url.rstrip('/')
        agent_url = f"{gateway_base}/agents/{deployment_name}"
        
        await self.register_agent_in_registry(agent_name, agent_url, owner_id, base_url, agent_path)
        
        if deployment_id:
            await self.update_deployment_status(deployment_id, base_url, "running", service_url=agent_url, agent_id=agent_name)
        
        await self.set_agent_status(agent_name, 'updated', {
            'message': f'Successfully updated to version {new_version}',
            'stage': 'update_deployed',
            'url': agent_url,
            'active_version': new_version,
            'deployment_name': deployment_name
        })

    async def _cleanup_old_agent_deployments(self, agent_id: str | None, version: str | None, keep_latest: int = 0):
        """Clean up old K8s deployments for an agent"""
        try:
            if not agent_id:
                return
            
            # Find old deployments
            old_deployments = self.k8s_service.list_agent_deployments(agent_id)
            
            # Filter by version if specified
            if version:
                old_deployments = [d for d in old_deployments if f"-v{version}-" in d or d.endswith(f"-{version}")]
            
            # Keep the latest N deployments
            if keep_latest > 0:
                old_deployments = sorted(old_deployments)[:-keep_latest]
            
            cleaned_count = 0
            for deployment_name in old_deployments:
                try:
                    if self.k8s_service.delete_agent_deployment(deployment_name):
                        cleaned_count += 1
                        self.logger.info(f"CLEANUP: Deleted old deployment: {deployment_name}")
                    else:
                        self.logger.warning(f"CLEANUP: Failed to delete deployment: {deployment_name}")
                except Exception as e:
                    self.logger.error(f"CLEANUP: Error deleting deployment {deployment_name}: {e}")
            
            self.logger.info(f"CLEANUP: Cleaned up {cleaned_count} old deployments for agent {agent_id}")
            
        except Exception as e:
            self.logger.error(f"CLEANUP: Error cleaning up deployments for {agent_id}: {e}")

    async def _update_registry_version_status(self, agent_name: str, status: str, base_url: str):
        """Update registry version status (e.g., from 'building' to 'active')"""
        try:
            url = f"{base_url}/api/v1/registry/agent/{agent_name}/version/status"
            update_data = {"status": status}
            
            async with aiohttp.ClientSession() as session:
                async with session.put(url, json=update_data, timeout=aiohttp.ClientTimeout(total=10)) as response:
                    if response.status == 200:
                        self.logger.info(f"Updated registry version status for {agent_name} to {status}")
                    else:
                        self.logger.warning(f"Failed to update registry version status for {agent_name}: {response.status}")
                        
        except Exception as e:
            self.logger.error(f"Error updating registry version status for {agent_name}: {e}")

    async def _finalize_agent_update(self, agent_id: str | None, new_version: str, previous_version: str | None, base_url: str):
        """Finalize agent update by updating version tracking"""
        try:
            # This could call an API to update registry version history
            # For now, just log the completion
            self.logger.info(f"FINALIZE_UPDATE: Agent {agent_id} updated from {previous_version} to {new_version}")
            
            # TODO: Update registry version_history via API if needed
            # url = f"{base_url}/api/v1/registry/agent/{agent_id}/version"
            # await update_version_history_api_call()
            
        except Exception as e:
            self.logger.error(f"FINALIZE_UPDATE: Error finalizing update for {agent_id}: {e}")

    async def _resolve_version_to_image_tag(self, agent_name: str, semantic_version: str, base_url: str) -> str:
        """
        Resolve semantic version to actual Docker image tag using version mapping.
        
        Args:
            agent_name: Name of the agent
            semantic_version: Semantic version like "2.0.0" 
            base_url: Backend API base URL
            
        Returns:
            Docker image tag (e.g., "v1736686234") or fallback to semantic version
        """
        try:
            # Query build records to find the image tag for the semantic version
            url = f"{base_url}/api/v1/agents/build/version-mapping"
            params = {
                "agent_id": agent_name,
                "semantic_version": semantic_version
            }
            
            async with aiohttp.ClientSession() as session:
                async with session.get(url, params=params, timeout=aiohttp.ClientTimeout(total=10)) as response:
                    if response.status == 200:
                        result = await response.json()
                        image_tag = result.get("image_tag")
                        if image_tag:
                            self.logger.info(f"VERSION_MAPPING: Resolved {semantic_version} → {image_tag}")
                            return image_tag
                    else:
                        self.logger.warning(f"Failed to resolve version mapping for {agent_name} v{semantic_version}: {response.status}")
                        
        except Exception as e:
            self.logger.error(f"Error resolving version mapping for {agent_name} v{semantic_version}: {e}")
        
        # Fallback: use semantic version as image tag (for backward compatibility)
        fallback_tag = f"v{semantic_version}"
        self.logger.warning(f"VERSION_MAPPING: Using fallback tag {fallback_tag} for {semantic_version}")
        return fallback_tag

    async def _inject_observability_if_enabled(self, agent_name: str, base_url: str, agent_path: str) -> str | None:
        """Inject observability code into agent if enabled"""
        if not self.observability_config.get_injection_enabled():
            self.logger.info(f"Observability injection disabled, skipping for {agent_name}")
            return None
        
        if not self.observability_config.is_tracing_enabled():
            self.logger.info(f"Tracing disabled, skipping injection for {agent_name}")
            return None
            
        try:
            import tempfile
            import tarfile
            
            self.logger.info(f"🔄 Starting observability injection for {agent_name}")
            
            # Step 1: Download agent files
            download_url = f"{base_url}/api/v1/agents/{agent_name}/download"
            if agent_path and agent_path.endswith(tuple(f'/v{i}' for i in range(10))) or '/v' in agent_path and agent_path.split('/v')[-1].replace('.', '').replace('-', '').isdigit():
                # Use versioned download only for proper version paths (e.g., /v1.0.0, /v2)
                # Skip version extraction for N8N agents or other non-versioned agents
                version = agent_path.split('/v')[-1] if '/v' in agent_path else None
                if version and (version.replace('.', '').replace('-', '').isdigit() or version.split('.')[0].isdigit()):
                    download_url = f"{base_url}/api/v1/agents/{agent_name}/download?version={version}"
                    
            async with aiohttp.ClientSession() as session:
                async with session.get(download_url, timeout=aiohttp.ClientTimeout(total=30)) as response:
                    if response.status != 200:
                        self.logger.warning(f"Failed to download agent files for injection: HTTP {response.status}")
                        return None
                    
                    # Save tarball to temp file
                    with tempfile.NamedTemporaryFile(mode='wb', suffix='.tar.gz', delete=False) as tmp_tar:
                        tmp_tar.write(await response.read())
                        tar_path = tmp_tar.name
            
            # Step 2: Extract and inject observability
            with tempfile.TemporaryDirectory() as extract_dir:
                self.logger.info(f"Extracting agent files for injection to {extract_dir}")
                with tarfile.open(tar_path, 'r:gz') as tar:
                    tar.extractall(extract_dir)
                
                # Check if Dockerfile exists before injection
                dockerfile_before = os.path.join(extract_dir, 'Dockerfile')
                dockerfile_exists_before = os.path.exists(dockerfile_before)
                self.logger.info(f"📋 Dockerfile exists before injection: {dockerfile_exists_before}")
                
                # Inject observability code
                injection_success = self.tracing_injector.inject_into_agent(extract_dir, agent_name)
                
                # Check if Dockerfile exists after injection
                dockerfile_exists_after = os.path.exists(dockerfile_before)
                dockerfile_size = os.path.getsize(dockerfile_before) if dockerfile_exists_after else 0
                self.logger.info(f"📋 Dockerfile exists after injection: {dockerfile_exists_after}, size: {dockerfile_size} bytes")
                
                if dockerfile_exists_before and not dockerfile_exists_after:
                    self.logger.error(f"🚨 Dockerfile was deleted during injection!")
                elif dockerfile_exists_after and dockerfile_size == 0:
                    self.logger.error(f"🚨 Dockerfile was corrupted during injection (0 bytes)!")
                
                if injection_success and dockerfile_exists_after and dockerfile_size > 0:                    # Step 3: Create ConfigMap with observability-injected files
                    import base64
                    
                    configmap_name = f"agent-files-{agent_name}-{int(time.time())}"
                    configmap_data = {}
                    
                    # Encode each file as base64 in the ConfigMap
                    for root, dirs, files in os.walk(extract_dir):
                        for file in files:
                            file_path = os.path.join(root, file)
                            # Create relative path for ConfigMap key
                            rel_path = os.path.relpath(file_path, extract_dir)
                            # Replace path separators with underscores for ConfigMap key
                            # Use base64 encoding to safely handle all file paths including dunder files
                            configmap_key = base64.b64encode(rel_path.encode('utf-8')).decode('ascii').replace('=', '_eq_').replace('+', '_plus_').replace('/', '_slash_')
                            
                            with open(file_path, 'rb') as f:
                                file_content = f.read()
                                configmap_data[configmap_key] = base64.b64encode(file_content).decode('utf-8')
                    
                    # Create ConfigMap using k8s_service
                    configmap_created = await self._create_agent_files_configmap(configmap_name, configmap_data)
                    
                    if configmap_created:
                        self.logger.info(f"✅ Created ConfigMap {configmap_name} with observability-injected files")
                        
                        # Cleanup original tar file
                        os.unlink(tar_path)
                        
                        # Return ConfigMap name for build job to use
                        return configmap_name
                    else:
                        self.logger.warning(f"Failed to create ConfigMap for {agent_name}, using original files")
                        # Cleanup and return None to use original files
                        os.unlink(tar_path)
                        return None
                else:
                    self.logger.warning(f"Observability injection failed for {agent_name}, continuing with original files")
                    # Cleanup and return None to use original files
                    os.unlink(tar_path)
                    return None
            
        except Exception as e:
            self.logger.error(f"Error during observability injection for {agent_name}: {e}")
            # Don't raise - continue with original files if injection fails
    


    async def _create_agent_files_configmap(self, configmap_name: str, configmap_data: dict) -> bool:
        """Create ConfigMap with agent files for build job"""
        try:
            success = await asyncio.to_thread(
                self.k8s_service.create_configmap_with_files,
                configmap_name,
                configmap_data,
                "nasiko-agents"  # namespace
            )
            return success
        except Exception as e:
            self.logger.error(f"Failed to create ConfigMap {configmap_name}: {e}")
            return False

    async def get_observability_env_vars(self, agent_name: str) -> dict:
        """Get environment variables for observability"""
        return {
            "PHOENIX_COLLECTOR_ENDPOINT": self.observability_config.get_phoenix_endpoint(),
            "TRACING_ENABLED": str(self.observability_config.is_tracing_enabled()).lower(),
            "AGENT_PROJECT_NAME": agent_name
        }

    async def acknowledge_message(self, msg_id: str):
        """Acknowledge message processing"""
        try:
            self.redis_client.xack(self.stream_name, self.consumer_group, msg_id)
            self.logger.debug(f"Acknowledged message {msg_id}")
        except Exception as e:
            self.logger.error(f"Failed to acknowledge message {msg_id}: {e}")

    def stop(self):
        """Stop the worker"""
        self.running = False


async def main():
    """Main function to run the K8s Build Worker"""
    logging.basicConfig(
        level=logging.INFO,
        format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
    )
    logger = logging.getLogger(__name__)

    logger.info("Starting K8s Build Worker...")
    logger.info(f"Registry URL: {settings.REGISTRY_URL}")
    logger.info(f"BuildKit Address: {settings.BUILDKIT_ADDRESS}")
    logger.info(f"Redis: {settings.REDIS_HOST}:{settings.REDIS_PORT}")

    worker = K8sBuildWorker(logger)
    await worker.start_listening()


if __name__ == "__main__":
    asyncio.run(main())
