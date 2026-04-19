"""
Agent Builder
Handles agent instrumentation, building, and deployment.
"""

import tempfile
import shutil
import yaml
import logging
import asyncio
import re
from pathlib import Path
from docker_utils import run_cmd
from registry_manager import RegistryManager
from instrumentation_injector import InstrumentationInjector
from config import AGENTS_DIRECTORY, DOCKER_NETWORK, Config

logger = logging.getLogger(__name__)


class AgentBuilder:
    """Handles building and deploying agents with instrumentation"""

    def __init__(self, logger=None):
        self.agents_dir = Path(AGENTS_DIRECTORY)
        self.registry_manager = RegistryManager()
        self.injector = InstrumentationInjector()
        self.logger = logger or logging.getLogger(__name__)

    def instrument_and_build_agents(self, owner_id=None):
        """Instrument and build all agents"""
        if not self.agents_dir.exists():
            logger.error(f"Agents directory {AGENTS_DIRECTORY} not found")
            return False

        success_count = 0
        total_count = 0

        for agent_folder in self.agents_dir.iterdir():
            if not agent_folder.is_dir():
                continue

            total_count += 1

            if self.build_single_agent(agent_folder.name, owner_id=owner_id):
                success_count += 1

        logger.info(f"Successfully built {success_count}/{total_count} agents")
        return success_count == total_count

    def build_single_agent(self, agent_folder_name, owner_id=None):
        """Build a single agent with instrumentation"""
        agent_folder = self.agents_dir / agent_folder_name

        if not agent_folder.is_dir():
            logger.error(f"Agent folder not found: {agent_folder}")
            return False

        # Validate docker-compose.yml exists and container names match
        if not self._validate_agent_structure(agent_folder):
            return False

        logger.info(f"Building agent: {agent_folder_name}")

        try:
            # Create temp directory and copy agent files
            temp_dir = Path(tempfile.mkdtemp())
            agent_temp_path = temp_dir / agent_folder_name
            shutil.copytree(agent_folder, agent_temp_path)

            # Build instrumented Docker image
            if not self._build_instrumented_image(
                agent_temp_path, agent_folder_name, None
            ):
                return False

            # Deploy agent with updated compose
            if not self._deploy_agent(agent_temp_path, agent_folder_name):
                return False

            # Update agent registry
            registry_result = self.registry_manager.update_agent_registry(
                agent_folder_name, action="upsert", owner_id=owner_id
            )

            # Cleanup temp directory
            shutil.rmtree(temp_dir)

            if registry_result.get("success", False):
                logger.info(
                    f"Successfully built and registered agent: {agent_folder_name}"
                )
                logger.info(f"Agent URL: {registry_result.get('url')}")
            else:
                logger.warning(
                    f"Agent built but registry update failed: {agent_folder_name}"
                )

            return True

        except Exception as e:
            logger.error(f"Error building agent {agent_folder_name}: {str(e)}")
            return False

    async def build_and_deploy_agent(
        self,
        agent_name: str,
        agent_path: str,
        base_url: str = "http://localhost:8000",
        owner_id=None,
    ):
        """
        Async method to build and deploy a single agent

        Args:
            agent_name: Name of the agent
            agent_path: Full path to agent directory on host
            base_url: Base URL for agent service
            owner_id: Owner ID

        Returns:
            Dict with success status and details
        """
        try:
            self.logger.info(
                f"Starting build and deploy for agent '{agent_name}' at '{agent_path}'"
            )

            agent_folder = Path(agent_path)

            if not agent_folder.exists() or not agent_folder.is_dir():
                return {
                    "success": False,
                    "error": f"Agent directory does not exist: {agent_path}",
                }

            # Validate agent structure
            if not self._validate_agent_structure(agent_folder):
                return {
                    "success": False,
                    "error": f"Invalid agent structure for {agent_name}",
                }

            # Run the build in executor to avoid blocking
            loop = asyncio.get_event_loop()
            result = await loop.run_in_executor(
                None, self._build_agent_sync, agent_name, agent_path, base_url, owner_id
            )

            return result

        except Exception as e:
            self.logger.error(f"Error in build_and_deploy_agent for {agent_name}: {e}")
            return {"success": False, "error": f"Build and deploy failed: {str(e)}"}

    def _build_agent_sync(
        self, agent_name: str, agent_path: str, base_url: str, owner_id=None
    ):
        """Synchronous method to build and deploy agent"""
        try:
            agent_folder = Path(agent_path)

            # Create temp directory and copy agent files
            temp_dir = Path(tempfile.mkdtemp())
            agent_temp_path = temp_dir / agent_name
            shutil.copytree(agent_folder, agent_temp_path)

            # Build instrumented Docker image
            if not self._build_instrumented_image(agent_temp_path, agent_name, None):
                shutil.rmtree(temp_dir)
                return {
                    "success": False,
                    "error": f"Failed to build Docker image for {agent_name}",
                }

            # Deploy agent with updated compose
            if not self._deploy_agent(agent_temp_path, agent_name):
                shutil.rmtree(temp_dir)
                return {
                    "success": False,
                    "error": f"Failed to deploy agent {agent_name}",
                }

            # Update agent registry
            registry_result = self.registry_manager.update_agent_registry(
                agent_name, action="upsert", owner_id=owner_id
            )

            # Cleanup temp directory
            shutil.rmtree(temp_dir)

            # Get agent URL from registry result (the actual URL from container port)
            url = registry_result.get("url") or f"{base_url}/agents/{agent_name}"
            registry_success = registry_result.get("success", False)
            registry_id = registry_result.get("registry_id")

            result = {
                "success": True,
                "agent_name": agent_name,
                "url": url,
                "service_name": agent_name,
                "container_id": None,  # Could be retrieved from docker inspect if needed
                "registry_updated": registry_success,
                "registry_id": registry_id,
            }

            if registry_success:
                self.logger.info(
                    f"Successfully built and registered agent: {agent_name}"
                )
                self.logger.info(f"Agent URL: {url}")
                if registry_id:
                    self.logger.info(f"Registry ID: {registry_id}")
            else:
                self.logger.warning(
                    f"Agent built but registry update failed: {agent_name}"
                )
                result["warning"] = "Registry update failed"

            return result

        except Exception as e:
            self.logger.error(f"Error in _build_agent_sync for {agent_name}: {e}")
            return {"success": False, "error": f"Synchronous build failed: {str(e)}"}

    def _validate_agent_structure(self, agent_folder):
        """Validate agent has required structure and container name matches folder name"""
        agent_folder_name = agent_folder.name
        compose_path = agent_folder / "docker-compose.yml"

        if not compose_path.exists():
            logger.error(
                f"No docker-compose.yml found for {agent_folder_name}, skipping..."
            )
            return False

        # Validate docker-compose.yml has valid structure and container names
        try:
            with open(compose_path, "r") as f:
                compose_data = yaml.safe_load(f)

            # Check if services section exists
            services = compose_data.get("services", {})
            if not services:
                logger.error(
                    f"No services found in docker-compose.yml for {agent_folder_name}, skipping..."
                )
                return False

            # Check if agent folder name matches any container name
            container_names = []
            for service_name, service_config in services.items():
                container_name = service_config.get("container_name", service_name)
                container_names.append(container_name)

            # Enforce that folder name matches at least one container name
            if agent_folder_name not in container_names:
                logger.error(
                    f"Agent folder name '{agent_folder_name}' must match one of the container names {container_names}"
                )
                return False

            logger.info(
                f"Agent '{agent_folder_name}' has valid structure with {len(services)} service(s) and matching container name"
            )
            return True

        except Exception as e:
            logger.error(
                f"Error reading docker-compose.yml for {agent_folder_name}: {e}, skipping..."
            )
            return False

    def _build_instrumented_image(
        self, agent_temp_path, agent_folder_name, agent_api_key
    ):
        """Build Docker image with instrumentation"""
        dockerfile_path = agent_temp_path / "Dockerfile"
        if not dockerfile_path.exists():
            logger.error(f"No Dockerfile found for {agent_folder_name}, skipping...")
            return False

        try:
            is_mcp = self._is_mcp_artifact(agent_temp_path)
            if is_mcp:
                bridge_src = Path(__file__).parent / "mcp_bridge.py"
                if bridge_src.exists():
                    logger.info("Injecting mcp_bridge.py into artifact")
                    shutil.copy(bridge_src, agent_temp_path / "mcp_bridge.py")
                self._ensure_mcp_dockerfile_runtime(agent_temp_path)

            # Inject LangTrace Configuration
            self.injector.inject_langtrace_config(agent_temp_path, agent_folder_name)

            # Check if image already exists locally (optimization for re-deployments)
            image_tag = self._safe_image_tag(agent_folder_name)
            result = run_cmd(["docker", "image", "inspect", image_tag], check=False)

            if result.returncode == 0:
                logger.info(
                    f"Docker image already exists: {image_tag} - reusing cached image (fast path)"
                )
                return True

            logger.info(f"Building new instrumented image for {agent_folder_name}")

            dockerfile_content = dockerfile_path.read_text()

            # Inject comprehensive instrumentation packages
            instrumentation_install = f"""
            # Install exact versions from pyproject.toml
            RUN pip install uv uvicorn fastapi mcp sse-starlette \\
                "opentelemetry-distro>=0.57b0" \\
                opentelemetry-sdk \\
                "opentelemetry-exporter-otlp>=1.36.0" \\
                "opentelemetry-exporter-otlp-proto-http>=1.36.0" \\
                opentelemetry-instrumentation \\
                "opentelemetry-instrumentation-asgi>=0.57b0" \\
                "opentelemetry-instrumentation-fastapi>=0.57b0" \\
                opentelemetry-instrumentation-django \\
                opentelemetry-instrumentation-flask \\
                opentelemetry-instrumentation-requests \\
                opentelemetry-instrumentation-httpx \\
                opentelemetry-instrumentation-aiohttp-client \\
                opentelemetry-instrumentation-pymongo \\
                opentelemetry-instrumentation-psycopg2 \\
                opentelemetry-instrumentation-sqlalchemy \\
                opentelemetry-instrumentation-redis \\
                opentelemetry-instrumentation-boto3sqs \\
                
            ENV ROOT_PATH=/{agent_folder_name}
            """

            # Append the instrumentation packages and env vars
            dockerfile_content = dockerfile_content + "\n" + instrumentation_install
            dockerfile_path.write_text(dockerfile_content)

            # Build instrumented image with real-time output
            image_tag = self._safe_image_tag(agent_folder_name)
            logger.info(f"Building Docker image: {image_tag}")

            # Use subprocess directly for real-time output
            import subprocess

            process = subprocess.Popen(
                ["docker", "build", "-t", image_tag, str(agent_temp_path)],
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,
            )

            # Stream output in real-time
            output_lines = []
            while True:
                line = process.stdout.readline()
                if not line and process.poll() is not None:
                    break
                if line:
                    print(line.rstrip())  # Print to console in real-time
                    output_lines.append(line.rstrip())

            return_code = process.poll()

            if return_code == 0:
                logger.info(f"Successfully built instrumented image: {image_tag}")
                return True
            else:
                logger.error(
                    f"Failed to build image for {agent_folder_name} (exit code: {return_code})"
                )
                # Full output is already printed, but log the last few lines for context
                if output_lines:
                    logger.error("Last few lines of build output:")
                    for line in output_lines[-10:]:  # Show last 10 lines
                        logger.error(f"  {line}")
                return False

        except Exception as e:
            logger.error(
                f"Error building instrumented image for {agent_folder_name}: {e}"
            )
            return False

    def _deploy_agent(self, agent_temp_path, agent_folder_name):
        """Deploy agent using docker-compose"""
        compose_path = agent_temp_path / "docker-compose.yml"

        if not compose_path.exists():
            logger.error(
                f"No docker-compose.yml found for {agent_folder_name}, skipping deployment"
            )
            return False

        try:
            # Load compose file
            with open(compose_path, "r") as f:
                compose_data = yaml.safe_load(f)

            # Ensure networks section exists
            if "networks" not in compose_data:
                compose_data["networks"] = {}

            # Add agents network
            compose_data["networks"]["agents-net"] = {
                "external": True,
                "name": DOCKER_NETWORK,
            }

            # Attach services to agents network & preserve original networks
            for _, svc_def in compose_data.get("services", {}).items():
                if "networks" not in svc_def:
                    svc_def["networks"] = []

                # Convert dict to list if needed
                if isinstance(svc_def["networks"], dict):
                    svc_def["networks"] = list(svc_def["networks"].keys())

                # Ensure agents network is attached
                if DOCKER_NETWORK not in svc_def["networks"]:
                    svc_def["networks"].append(DOCKER_NETWORK)

            # Update services to use pre-built instrumented image and inject API keys
            image_tag = self._safe_image_tag(agent_folder_name)
            is_mcp = self._is_mcp_artifact(agent_temp_path)
            
            api_key_env = {
                "OPENAI_API_KEY": Config.OPENAI_API_KEY,
                "OPENROUTER_API_KEY": Config.OPENROUTER_API_KEY,
                "MINIMAX_API_KEY": Config.MINIMAX_API_KEY,
            }
            for service_name, svc_def in compose_data.get("services", {}).items():
                is_main_agent = service_name == agent_folder_name or svc_def.get("container_name") == agent_folder_name
                
                if is_main_agent:
                    if "build" in svc_def:
                        svc_def.pop("build", None)
                    svc_def["image"] = image_tag
                    
                    if is_mcp:
                        svc_def["command"] = ["python", "mcp_bridge.py"]
                        env = svc_def.get("environment", [])
                        if isinstance(env, list) and not any(str(item).startswith("PORT=") for item in env):
                            env.append("PORT=5000")
                        elif isinstance(env, dict):
                            env.setdefault("PORT", "5000")
                        svc_def["environment"] = env

                # Inject actual API key values directly (bypasses yaml/shell substitution issues)
                env = svc_def.get("environment", [])
                if isinstance(env, list):
                    new_env = []
                    for item in env:
                        if isinstance(item, str):
                            key = item.split("=")[0]
                            if key in api_key_env and api_key_env[key]:
                                new_env.append(f"{key}={api_key_env[key]}")
                                continue
                        new_env.append(item)
                    svc_def["environment"] = new_env

            # Save updated compose
            with open(compose_path, "w") as f:
                yaml.dump(compose_data, f)

            # Deploy agent — use --env-file so docker compose loads the agent's .env
            # regardless of the process working directory (which is the nasiko root, not the agent dir)
            compose_cmd = [
                "docker",
                "compose",
                "-f",
                str(compose_path),
            ]
            env_file = agent_temp_path / ".env"
            if env_file.exists():
                compose_cmd.extend(["--env-file", str(env_file)])
                logger.info(f"Loading agent env file: {env_file}")
            compose_cmd.extend(["up", "-d"])
            result = run_cmd(
                compose_cmd, check=False
            )  # Don't raise exception on failure

            if result.returncode == 0:
                logger.info(f"Successfully deployed agent: {agent_folder_name}")
                return True
            else:
                logger.error(f"Failed to deploy agent {agent_folder_name}:")
                logger.error(f"Return code: {result.returncode}")
                if result.stdout:
                    logger.error(f"Docker compose stdout:\n{result.stdout}")
                if result.stderr:
                    logger.error(f"Docker compose stderr:\n{result.stderr}")
                return False

        except Exception as e:
            logger.error(f"Error deploying agent {agent_folder_name}: {e}")
            return False

    def _safe_image_tag(self, agent_name: str) -> str:
        safe_name = re.sub(r"[^a-z0-9_.-]+", "-", agent_name.lower()).strip(".-")
        return f"{safe_name or 'agent'}_instrumented"

    def _is_mcp_artifact(self, agent_path: Path) -> bool:
        if (agent_path / "McpServerManifest.json").exists():
            return True
        signatures = ("FastMCP", "mcp.server", "@mcp.tool", "@mcp.resource", "@mcp.prompt")
        for py_file in agent_path.rglob("*.py"):
            try:
                content = py_file.read_text(encoding="utf-8")
            except Exception:
                continue
            if any(signature in content for signature in signatures):
                return True
        return False

    def _ensure_mcp_dockerfile_runtime(self, agent_path: Path):
        dockerfile_path = agent_path / "Dockerfile"
        if not dockerfile_path.exists():
            return

        content = dockerfile_path.read_text()
        additions = []
        if "WORKDIR " not in content:
            additions.append("WORKDIR /app")
        if not re.search(r"^\s*(COPY|ADD)\s+(\.|\./|src|src/)", content, re.MULTILINE):
            additions.append("COPY . /app")
        if " mcp" not in content and "\nmcp" not in content and "fastapi" not in content.lower():
            additions.append(
                "RUN pip install --no-cache-dir fastapi uvicorn mcp sse-starlette "
                '"opentelemetry-api>=1.36.0"'
            )

        if additions:
            dockerfile_path.write_text(
                content.rstrip()
                + "\n\n# Nasiko MCP bridge runtime\n"
                + "\n".join(additions)
                + "\n"
            )
            logger.info("Updated MCP Dockerfile with bridge runtime requirements")
