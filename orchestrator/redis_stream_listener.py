"""
Redis Stream Listener for Orchestrator
Listens for orchestration commands from the backend app via Redis streams
"""

import redis
import json
import logging
import asyncio
import signal
import sys
import aiohttp
from typing import Dict, Any, Optional
from datetime import datetime, UTC
from pathlib import Path

from config import Config
from agent_builder import AgentBuilder

# Import observability components directly like K8s build worker
import sys
sys.path.insert(0, '/app')
from app.utils.observability.injector import TracingInjector
from app.utils.observability.config import ObservabilityConfig


class RedisStreamListener:
    """Redis stream listener for orchestration commands"""
    
    def __init__(self, logger: logging.Logger):
        self.logger = logger
        self.redis_client = None
        self.agent_builder = AgentBuilder(logger)
        self.running = False
        self.stream_name = "orchestration:commands"
        self.consumer_group = "orchestrator"
        self.consumer_name = "orchestrator-1"
        
        # Initialize observability components exactly like K8s build worker
        import app.utils.observability as observability_pkg
        observability_path = str(Path(observability_pkg.__file__).resolve().parent)
        self.tracing_injector = TracingInjector(observability_source_path=observability_path)
        self.observability_config = ObservabilityConfig()
        self.logger.info("Initialized observability components successfully")
        
    def connect_redis(self):
        """Connect to Redis server"""
        try:
            self.redis_client = redis.Redis(
                host=Config.REDIS_HOST,
                port=Config.REDIS_PORT,
                db=Config.REDIS_DB,
                decode_responses=True,
                socket_connect_timeout=10,
                socket_timeout=10
            )
            # Test connection
            self.redis_client.ping()
            self.logger.info(f"Connected to Redis at {Config.REDIS_HOST}:{Config.REDIS_PORT}")
            
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
        self.logger.info("Starting Redis stream listener for orchestration commands")
        
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
            self.logger.info("Redis stream listener stopped")
    
    async def process_message(self, msg_id: str, fields: Dict[str, str]):
        """Process orchestration command for local deployment from mounted agents folder"""
        try:
            self.logger.info(f"Processing message {msg_id}: {fields}")
            
            # Extract command details
            command = fields.get('command')
            agent_name = fields.get('agent_name')
            agent_path = fields.get('agent_path')  # Extract agent_path from message
            base_url = fields.get('base_url', 'http://localhost:8000')
            
            # Extract additional data from orchestration message
            owner_id = fields.get('owner_id')
            upload_id = fields.get('upload_id')
            upload_type = fields.get('upload_type')
            
            if not all([command, agent_name]):
                self.logger.error(f"Invalid message format: missing required fields (command, agent_name)")
                await self.acknowledge_message(msg_id)
                return
            
            # Log extracted ownership information
            if owner_id:
                self.logger.info(f"Processing agent '{agent_name}' for owner: {owner_id} (upload_id: {upload_id}, type: {upload_type})")
            
            # Set initial status in Redis and update database
            await self.set_agent_status(agent_name, 'processing', {
                'message': 'Orchestration command received',
                'stage': 'initializing',
                'owner_id': owner_id,
                'upload_id': upload_id,
                'upload_type': upload_type
            })
            await self.update_database_status(agent_name, base_url, 'orchestration_processing', 5, 
                                            'Orchestration processing started')
            
            # Process based on command type - both deploy and update use same handler
            if command in ['deploy_agent', 'update_agent']:
                await self.handle_agent_deployment(command, agent_name, base_url, owner_id, upload_id, upload_type, agent_path)
            else:
                self.logger.warning(f"Unknown command: {command}")
                await self.set_agent_status(agent_name, 'error', {
                    'message': f'Unknown command: {command}',
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
                                 owner_id: Optional[str] = None, upload_id: Optional[str] = None, 
                                 upload_type: Optional[str] = None):
        """Handle agent deployment command"""
        try:
            if owner_id:
                self.logger.info(f"Deploying agent '{agent_name}' from path '{agent_path}' for owner: {owner_id}")
            else:
                self.logger.info(f"Deploying agent '{agent_name}' from path '{agent_path}'")
            
            # Convert container path to host path
            # Container path: /app/agents/{agent_name}
            # Host path: agents/{agent_name}
            if agent_path.startswith('/app/agents/'):
                relative_path = agent_path.replace('/app/agents/', '')
                host_agent_path = str(Path('agents') / relative_path) # TODO - replace with config value
            else:
                host_agent_path = agent_path

            # Verify agent directory exists
            agent_dir = Path(host_agent_path)
            if not agent_dir.exists():
                raise ValueError(f"Agent directory does not exist: {host_agent_path}")
            
            # Set status to building (include ownership info)
            await self.set_agent_status(agent_name, 'building', {
                'message': 'Building Docker image',
                'stage': 'docker_build',
                'owner_id': owner_id,
                'upload_id': upload_id,
                'upload_type': upload_type
            })
            
            # Build and deploy agent using AgentBuilder
            result = await self.agent_builder.build_and_deploy_agent(
                agent_name=agent_name,
                agent_path=host_agent_path,
                base_url=base_url,
                owner_id=owner_id
            )
            
            if result.get('success', False):
                # Set status to running in Redis
                await self.set_agent_status(agent_name, 'running', {
                    'message': 'Agent deployed successfully',
                    'stage': 'deployed',
                    'url': result.get('url'),
                    'container_id': result.get('container_id'),
                    'service_name': result.get('service_name')
                })
                
                # Update database status to completed
                await self.update_database_status(agent_name, base_url, 'completed', 100, 
                                                'Agent deployed and running successfully', {
                                                    'url': result.get('url'),
                                                    'registry_updated': True
                                                })
                self.logger.info(f"Successfully deployed agent '{agent_name}'")
            else:
                # Set status to failed in Redis
                await self.set_agent_status(agent_name, 'failed', {
                    'message': result.get('error', 'Unknown deployment error'),
                    'stage': 'deployment_failed'
                })
                
                # Update database status to failed
                await self.update_database_status(agent_name, base_url, 'failed', 0, 
                                                f"Deployment failed: {result.get('error', 'Unknown error')}", {
                                                    'error_details': [result.get('error', 'Unknown deployment error')]
                                                })
                self.logger.error(f"Failed to deploy agent '{agent_name}': {result.get('error')}")
                
        except Exception as e:
            self.logger.error(f"Error deploying agent '{agent_name}': {e}")
            await self.set_agent_status(agent_name, 'failed', {
                'message': f'Deployment failed: {str(e)}',
                'stage': 'deployment_error'
            })
    
    async def set_agent_status(self, agent_name: str, status: str, details: Optional[Dict[str, Any]] = None):
        """Set agent deployment status in Redis"""
        if not self.is_connected():
            return
        
        try:
            status_key = f"agent:status:{agent_name}"
            status_data = {
                "agent_name": agent_name,
                "status": status,
                "last_updated": datetime.now(UTC).isoformat(),
                "updated_by": "orchestrator"
            }
            
            if details:
                # Filter out None values and convert values to Redis-compatible types
                filtered_details = {}
                for k, v in details.items():
                    if v is not None:
                        # Convert boolean values to strings for Redis storage
                        if isinstance(v, bool):
                            filtered_details[k] = str(v).lower()
                        else:
                            filtered_details[k] = v
                status_data.update(filtered_details)
            
            # Store as hash for easy field access
            self.redis_client.hset(status_key, mapping=status_data)
            
            # Set expiration (24 hours)
            self.redis_client.expire(status_key, 86400)
            
            self.logger.debug(f"Set agent status for {agent_name}: {status}")
            
        except Exception as e:
            self.logger.error(f"Failed to set agent status for {agent_name}: {e}")
    
    async def update_database_status(self, agent_name: str, base_url: str, status: str, 
                                   progress: int, message: str, additional_data: Optional[Dict[str, Any]] = None):
        """Update upload status in database via API call to the backend"""
        try:
            update_data = {
                "status": status,
                "progress_percentage": progress,
                "status_message": message,
                "orchestration_duration": None  # Could be calculated if needed
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

    async def acknowledge_message(self, msg_id: str):
        """Acknowledge message processing"""
        try:
            self.redis_client.xack(self.stream_name, self.consumer_group, msg_id)
            self.logger.debug(f"Acknowledged message {msg_id}")
        except Exception as e:
            self.logger.error(f"Failed to acknowledge message {msg_id}: {e}")
    
    async def handle_agent_deployment(self, command: str, agent_name: str, base_url: str,
                                     owner_id: Optional[str] = None, upload_id: Optional[str] = None, 
                                     upload_type: Optional[str] = None, agent_path: Optional[str] = None):
        """Handle both agent deployment and updates from mounted agents folder"""
        
        try:
            self.logger.info(f"{command.title()}ing agent '{agent_name}' for owner: {owner_id}")
            
            # Step 1: Verify agent source exists in mounted folder using provided agent_path
            if agent_path:
                # Use the agent_path from the message (e.g., /app/agents/a2a-translator/v1.0.0)
                agent_source_path = Path(agent_path)
            else:
                # Fallback to the old method if no agent_path provided
                agent_source_path = self._get_agent_source_path(agent_name)
            
            # Verify the path exists and has required files
            if not agent_source_path.exists():
                raise ValueError(f"Agent source not found: {agent_source_path}")
            
            if not agent_source_path.is_dir():
                raise ValueError(f"Agent path is not a directory: {agent_source_path}")
            
            # Verify required files exist
            dockerfile_path = agent_source_path / "Dockerfile"
            agentcard_path = agent_source_path / "Agentcard.json"
            
            if not dockerfile_path.exists():
                raise ValueError(f"Dockerfile not found in {agent_source_path}")
            
            if not agentcard_path.exists():
                self.logger.warning(f"Agentcard.json not found in {agent_source_path}")
            
            # Step 2: Stop existing agent if updating
            if command == 'update_agent':
                await self._stop_existing_agent(agent_name)
            
            # Step 3: Build Docker image from mounted source
            await self._update_status(agent_name, 'building', 'Building Docker image', owner_id, upload_id, base_url, 25)
            image_tag = await self._build_local_docker_image(agent_source_path, agent_name)
            
            # Step 4: Deploy agent container  
            await self._update_status(agent_name, 'deploying', 'Deploying agent container', owner_id, upload_id, base_url, 50)
            deployment_result = await self._deploy_agent_container(agent_name, image_tag, owner_id, upload_type)
            
            # Step 5: Update agent registry via backend API
            await self._update_status(agent_name, 'registering', 'Updating agent registry', owner_id, upload_id, base_url, 90)
            registry_result = await self._update_agent_registry_with_path(agent_name, deployment_result, owner_id, base_url, agent_source_path)
            
            # Step 5.5: Create Agent Permissions (if registry was updated and owner_id is provided)
            permissions_created = False
            if registry_result.get('success', False) and owner_id:
                self.logger.info(f"Creating permissions for agent {agent_name} with owner {owner_id}")
                permissions_created = await self.create_agent_permissions(agent_name, owner_id)
                if not permissions_created:
                    self.logger.warning(f"Registry updated but permission creation failed for agent {agent_name}")
            elif registry_result.get('success', False) and not owner_id:
                self.logger.info(f"Registry updated for agent {agent_name} but no owner_id provided, skipping permissions")
            
            # Step 6: Final status update
            # Use Kong gateway URL for external access
            container_name = deployment_result.get('container_name', f"agent-{agent_name}")
            external_url = f"{Config.KONG_GATEWAY_URL}/agents/{container_name}"
            
            await self._update_status(agent_name, 'completed', 'Agent ready and registered', owner_id, upload_id, base_url, 100, {
                'url': external_url,
                'container_id': deployment_result['container_id'],
                'registry_id': registry_result.get('registry_id'),
                'permissions_created': permissions_created
            })
            
            self.logger.info(f"Successfully {command}ed agent '{agent_name}' at {external_url}")
            
        except Exception as e:
            await self._update_status(agent_name, 'failed', f"{command.title()} failed: {str(e)}", owner_id, upload_id, base_url, 0)
            raise

    def _get_agent_source_path(self, agent_name: str) -> Path:
        """Get agent source path from mounted agents folder"""
        
        # Path inside worker container (mounted from backend)
        agents_base = Path("/app/agents")
        agent_path = agents_base / agent_name
        
        if not agent_path.exists():
            raise ValueError(f"Agent source not found: {agent_path}")
        
        if not agent_path.is_dir():
            raise ValueError(f"Agent path is not a directory: {agent_path}")
        
        # Verify required files exist
        dockerfile_path = agent_path / "Dockerfile"
        agentcard_path = agent_path / "Agentcard.json"
        
        if not dockerfile_path.exists():
            raise ValueError(f"Dockerfile not found in {agent_path}")
        
        if not agentcard_path.exists():
            self.logger.warning(f"Agentcard.json not found in {agent_path}")
        
        return agent_path

    async def _stop_existing_agent(self, agent_name: str):
        """Stop existing agent for update"""
        await self._cleanup_existing_container(agent_name)
        self.logger.info(f"Stopped existing agent for update: {agent_name}")

    async def _update_status(self, agent_name: str, status: str, message: str, 
                            owner_id: Optional[str] = None, upload_id: Optional[str] = None,
                            base_url: str = 'http://localhost:8000', progress: int = 50,
                            additional_data: Optional[Dict[str, Any]] = None):
        """Update agent status in Redis and backend database"""
        
        # Update Redis status
        await self.set_agent_status(agent_name, status, {
            'message': message,
            'stage': status,
            'owner_id': owner_id,
            'upload_id': upload_id,
            **(additional_data or {})
        })
        
        # Update database via backend API
        await self.update_database_status(
            agent_name,
            base_url,
            status,
            progress,
            message,
            additional_data
        )

    async def _build_local_docker_image(self, source_path: Path, agent_name: str) -> str:
        """Build Docker image with observability injection"""
        import shutil
        import time
        
        # Create temporary build directory with instrumentation
        temp_dir = Path(f"/tmp/agent-builds/{agent_name}-{int(time.time())}")
        temp_dir.mkdir(parents=True, exist_ok=True)
        
        try:
            # Copy agent source to temp directory
            shutil.copytree(source_path, temp_dir / "agent", dirs_exist_ok=True)
            
            # Inject observability (like existing agent_builder.py does)
            await self._inject_observability(temp_dir / "agent", agent_name)
            
            # Build Docker image
            image_tag = f"local-agent-{agent_name}:latest"
            
            build_cmd = [
                'docker', 'build',
                '-t', image_tag,
                str(temp_dir / "agent")
            ]
            
            process = await asyncio.create_subprocess_exec(
                *build_cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE
            )
            
            stdout, stderr = await process.communicate()
            
            if process.returncode != 0:
                error_msg = stderr.decode() if stderr else "Unknown build error"
                raise Exception(f"Docker build failed: {error_msg}")
            
            self.logger.info(f"Successfully built image: {image_tag}")
            return image_tag
            
        finally:
            # Cleanup temp directory
            if temp_dir.exists():
                shutil.rmtree(temp_dir)

    async def _inject_observability(self, agent_path: Path, agent_name: str):
        """Inject observability code exactly like K8s build worker"""
        if not self.observability_config.get_injection_enabled():
            self.logger.info(f"Observability injection disabled, skipping for {agent_name}")
            return
        
        if not self.observability_config.is_tracing_enabled():
            self.logger.info(f"Tracing disabled, skipping injection for {agent_name}")
            return
            
        try:
            self.logger.info(f"🔄 Starting observability injection for {agent_name}")
            
            # Check if Dockerfile exists before injection
            dockerfile_before = agent_path / 'Dockerfile'
            dockerfile_exists_before = dockerfile_before.exists()
            self.logger.info(f"📋 Dockerfile exists before injection: {dockerfile_exists_before}")
            
            # Inject observability code using TracingInjector
            injection_success = self.tracing_injector.inject_into_agent(str(agent_path), agent_name)
            
            # Check if Dockerfile exists after injection
            dockerfile_exists_after = dockerfile_before.exists()
            dockerfile_size = dockerfile_before.stat().st_size if dockerfile_exists_after else 0
            self.logger.info(f"📋 Dockerfile exists after injection: {dockerfile_exists_after}, size: {dockerfile_size} bytes")
            
            if dockerfile_exists_before and not dockerfile_exists_after:
                self.logger.error(f"🚨 Dockerfile was deleted during injection!")
            elif dockerfile_exists_after and dockerfile_size == 0:
                self.logger.error(f"🚨 Dockerfile was corrupted during injection (0 bytes)!")
            
            if injection_success and dockerfile_exists_after and dockerfile_size > 0:
                self.logger.info(f"✅ Successfully injected observability for {agent_name}")
            else:
                self.logger.warning(f"Observability injection failed for {agent_name}, continuing with original files")
                
        except Exception as e:
            self.logger.error(f"Error during observability injection for {agent_name}: {e}")
            # Don't raise - continue with original files if injection fails

    async def _deploy_agent_container(self, agent_name: str, image_tag: str, owner_id: str, upload_type: Optional[str] = None, webhook_url: Optional[str] = None) -> dict:
        """Deploy agent container with proper networking"""
        import socket
        
        # Stop and remove existing container if it exists
        await self._cleanup_existing_container(agent_name)
        
        # Find available port
        port = await self._find_available_port()
        container_name = f"agent-{agent_name}"
        
        # Prepare environment variables like K8s worker
        env_vars = {
            'AGENT_NAME': agent_name,
            'OWNER_ID': owner_id or '',
            'OPENAI_API_KEY': Config.OPENAI_API_KEY
        }
        
        # Add observability environment variables
        obs_env_vars = await self.get_observability_env_vars(agent_name)
        env_vars.update(obs_env_vars)
        
        # Add WEBHOOK_URL for n8n agents  
        if upload_type == "n8n_register" and webhook_url:
            env_vars['WEBHOOK_URL'] = webhook_url
        
        # Build Docker run command with all environment variables
        docker_cmd = [
            'docker', 'run', '-d',
            '--name', container_name,
            '--network', Config.AGENTS_NETWORK,  # Join agents network for Kong discovery
            '--network', Config.APP_NETWORK,  # Also join app network for observability access
            '-p', f'{port}:5000',  # Expose on host for direct access (agent runs on 5000)
            '--restart', 'unless-stopped'
        ]
        
        # Add all environment variables to the command
        for key, value in env_vars.items():
            if value:  # Only add non-empty values
                docker_cmd.extend(['-e', f'{key}={value}'])
        
        docker_cmd.append(image_tag)
        
        process = await asyncio.create_subprocess_exec(
            *docker_cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE
        )
        
        stdout, stderr = await process.communicate()
        
        if process.returncode != 0:
            error_msg = stderr.decode() if stderr else "Unknown deployment error"
            raise Exception(f"Container deployment failed: {error_msg}")
        
        container_id = stdout.decode().strip()
        
        return {
            'container_id': container_id,
            'container_name': container_name,
            'port': port,
            'url': f'http://localhost:{port}',
            'network_url': f'http://{container_name}:5000'  # For internal network access (agent runs on 5000)
        }

    async def _cleanup_existing_container(self, agent_name: str):
        """Stop and remove existing container if it exists"""
        container_name = f"agent-{agent_name}"
        
        # Stop container
        await asyncio.create_subprocess_exec(
            'docker', 'stop', container_name,
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL
        )
        
        # Remove container
        await asyncio.create_subprocess_exec(
            'docker', 'rm', container_name,
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL
        )
        
        self.logger.debug(f"Cleaned up existing container: {container_name}")

    async def _find_available_port(self, start_port: int = 8010) -> int:
        """Find an available port for the agent"""
        import socket
        
        for port in range(start_port, start_port + 100):
            try:
                with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                    s.bind(('localhost', port))
                    return port
            except OSError:
                continue
        
        raise Exception("No available ports found")


    async def fetch_agentcard_from_local(self, agent_name: str, agent_path: Path) -> Dict[str, Any] | None:
        """
        Fetch AgentCard.json from local mounted agent directory.
        Mimics the K8s worker's fetch_agentcard_from_backend but reads from local filesystem.
        """
        try:
            # Look for Agentcard.json in the agent directory
            agentcard_path = agent_path / "Agentcard.json"

            if agentcard_path.exists():
                self.logger.info(f"Found Agentcard.json for {agent_name}")
                with open(agentcard_path, 'r') as f:
                    return json.load(f)
            else:
                self.logger.warning(f"Agentcard.json not found for {agent_name}, attempting to generate")
                return await self.generate_agentcard(str(agent_path), agent_name)

        except Exception as e:
            self.logger.error(f"Error fetching/generating Agentcard for {agent_name}: {e}")
            return None
    
    async def generate_agentcard(self, agent_path: str, agent_name: str) -> Dict[str, Any] | None:
        """Generate AgentCard using the AgentCard Generator (mimics K8s worker)"""
        try:
            from app.utils.agentcard_generator import AgentCardGeneratorAgent
            
            self.logger.info(f"Generating AgentCard for {agent_name} using AgentCard Generator")

            # Check if OPENAI_API_KEY is available from config
            openai_key = Config.OPENAI_API_KEY
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
        agent_path: Path | None = None,
    ) -> bool:
        """Register or update agent in the registry via API (mimics K8s worker)"""
        try:
            # Try to fetch AgentCard from local directory
            agentcard_data = None
            if agent_path:
                agentcard_data = await self.fetch_agentcard_from_local(agent_name, agent_path)
            
            if agentcard_data:
                self.logger.info(f"Using AgentCard data for {agent_name} with {len(agentcard_data.get('skills', []))} skills")

                # Use full AgentCard data (deep copy to avoid modifying original)
                registry_data = json.loads(json.dumps(agentcard_data))
                
                # Override/ensure critical local deployment fields
                registry_data["id"] = agent_name
                registry_data["url"] = service_url
                registry_data["deployment_type"] = "docker-local"
                
                if owner_id:
                    registry_data["owner_id"] = owner_id
            else:
                # Fallback to minimal registry entry
                self.logger.warning(f"No AgentCard found/generated for {agent_name}, using minimal capabilities")
                registry_data = {
                    "id": agent_name,
                    "name": agent_name,
                    "url": service_url,
                    "description": f"Agent deployed via local Docker",
                    "capabilities": {
                        "tools": [],
                        "prompts": []
                    },
                    "version": "1.0.0",
                    "deployment_type": "docker-local"
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

    async def _update_agent_registry_with_path(self, agent_name: str, deployment_result: dict, 
                                   owner_id: str, base_url: str, agent_source_path: Path) -> dict:
        """Update agent registry via backend API using new AgentCard-based registration"""
        try:
            # For local deployment, use Kong gateway URL like K8s worker does
            # Kong gateway runs at port 9100 for local deployment
            container_name = deployment_result.get('container_name', f"agent-{agent_name}")
            gateway_url = f"{Config.KONG_GATEWAY_URL}/agents/{container_name}"
            
            # Use the new registry registration method with the actual agent source path
            success = await self.register_agent_in_registry(
                agent_name=agent_name,
                service_url=gateway_url,
                owner_id=owner_id,
                base_url=base_url,
                agent_path=agent_source_path
            )
            
            if success:
                return {'success': True, 'registry_id': agent_name}
            else:
                return {'success': False, 'error': 'Registry update failed'}
                        
        except Exception as e:
            self.logger.error(f"Error updating agent registry: {e}")
            return {'success': False, 'error': str(e)}


    async def create_agent_permissions(self, agent_id: str, owner_id: str) -> bool:
        """Create agent permissions in the auth service (copied from K8s worker)"""
        try:
            # Use auth service URL from config - use container network name
            auth_service_url = f"http://nasiko-auth-service:8001"  # For local deployment
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

    async def get_observability_env_vars(self, agent_name: str) -> dict:
        """Get environment variables for observability (matches K8s worker)"""
        # Use local Phoenix endpoints (matching docker-compose.local.yml)
        return {
            "PHOENIX_COLLECTOR_ENDPOINT": "http://phoenix-observability:6006/v1/traces",
            "PHOENIX_GRPC_ENDPOINT": "http://phoenix-observability:4317", 
            "PHOENIX_HTTP_ENDPOINT": "http://phoenix-observability:6006",
            "TRACING_ENABLED": "true",
            "AGENT_PROJECT_NAME": agent_name
        }

    def stop(self):
        """Stop the listener"""
        self.running = False


async def main():
    """Main function to run the Redis stream listener"""
    logging.basicConfig(
        level=logging.INFO,
        format='%(asctime)s - %(levelname)s - %(message)s'
    )
    logger = logging.getLogger(__name__)
    
    listener = RedisStreamListener(logger)
    await listener.start_listening()


if __name__ == "__main__":
    asyncio.run(main())