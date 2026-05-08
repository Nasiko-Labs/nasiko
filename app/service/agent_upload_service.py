import os
import re
import shutil
import tempfile
import zipfile
from pathlib import Path
from typing import Optional, List

import yaml
from fastapi import UploadFile

from app.pkg.config.config import settings
from app.service.agentcard_service import AgentCardService
from app.service.orchestration_service import OrchestrationService


class AgentUploadResult:
    def __init__(
        self,
        success: bool,
        agent_name: str,
        status: str,
        capabilities_generated: bool = False,
        orchestration_triggered: bool = False,
        validation_errors: Optional[List[str]] = None,
        upload_id: Optional[str] = None,
        version: Optional[str] = None,
        artifact_type: Optional[str] = None,
    ):
        self.success = success
        self.agent_name = agent_name
        self.status = status
        self.capabilities_generated = capabilities_generated
        self.orchestration_triggered = orchestration_triggered
        self.validation_errors = validation_errors or []
        self.upload_id = upload_id
        self.version = version
        self.artifact_type = artifact_type


class ValidationResult:
    def __init__(
        self,
        is_valid: bool,
        errors: List[str] = None,
        artifact_type: Optional[str] = None,
    ):
        self.is_valid = is_valid
        self.errors = errors or []
        self.artifact_type = artifact_type or "agent"


async def _determine_agent_name(temp_dir: str) -> str:
    """Determine agent name from docker-compose.yml container names"""
    temp_path = Path(temp_dir)

    # First check for docker-compose.yml
    compose_path = temp_path / "docker-compose.yml"
    if compose_path.exists():
        try:
            with open(compose_path, "r") as f:
                compose_data = yaml.safe_load(f)

            services = compose_data.get("services", {})
            if services:
                # Get the first container name found
                for service_name, service_config in services.items():
                    container_name = service_config.get("container_name", service_name)
                    return container_name
        except Exception:
            pass  # Fall back to directory name if yaml parsing fails

    # Fallback to directory name
    return os.path.basename(temp_dir)


class AgentUploadService:
    def __init__(self, logger, repository=None):
        self.logger = logger
        self.agentcard_service = AgentCardService(logger)
        from app.utils.agentcard_generator.mcp_manifest_generator import (
            MCPManifestGeneratorAgent,
        )

        self.mcp_manifest_service = MCPManifestGeneratorAgent()
        self.orchestration = OrchestrationService(logger)
        self.agents_directory = Path("agents")
        self.repository = repository

    def _scan_file_for_markers(self, file_path: Path) -> tuple[bool, bool]:
        """
        Return (agent_marker_found, mcp_marker_found) for one python file.
        """
        try:
            content = file_path.read_text(encoding="utf-8", errors="ignore")
        except Exception:
            return False, False

        agent_patterns = [
            r"\bfrom\s+langchain\b",
            r"\bimport\s+langchain\b",
            r"\bfrom\s+crewai\b",
            r"\bimport\s+crewai\b",
            r"\bAgentCard\b",
            r"\ba2a\b",
        ]

        mcp_patterns = [
            r"\bfrom\s+fastmcp\b",
            r"\bimport\s+fastmcp\b",
            r"\bfrom\s+mcp\b",
            r"\bimport\s+mcp\b",
            r"@\s*tool\b",
            r"@\s*FastMCP\.tool\b",
            r"\bFastMCP\s*\(",
        ]

        agent_found = any(
            re.search(pattern, content, re.IGNORECASE) for pattern in agent_patterns
        )
        mcp_found = any(
            re.search(pattern, content, re.IGNORECASE) for pattern in mcp_patterns
        )

        return agent_found, mcp_found

    def _detect_artifact_type(self, agent_dir: Path) -> tuple[str, List[str]]:
        """
        Detect whether this upload is a standard agent or an MCP server.
        Returns: (artifact_type, errors)
        """
        errors: List[str] = []

        agent_markers = []
        mcp_markers = []

        # Manifest signals
        if (agent_dir / "AgentCard.json").exists() or (agent_dir / "Agentcard.json").exists():
            agent_markers.append("AgentCard.json")

        for mcp_name in ["MCPManifest.json", "mcp-manifest.json", "mcp_manifest.json"]:
            if (agent_dir / mcp_name).exists():
                mcp_markers.append(mcp_name)

        # Code signals
        python_files = list(agent_dir.rglob("*.py"))
        for py_file in python_files:
            agent_found, mcp_found = self._scan_file_for_markers(py_file)
            if agent_found:
                agent_markers.append(str(py_file.relative_to(agent_dir)))
            if mcp_found:
                mcp_markers.append(str(py_file.relative_to(agent_dir)))

        # Ambiguity check
        if agent_markers and mcp_markers:
            errors.append(
                "Ambiguous artifact detected: both standard agent markers and MCP server markers were found."
            )
            errors.append(f"Agent markers: {sorted(set(agent_markers))}")
            errors.append(f"MCP markers: {sorted(set(mcp_markers))}")
            return "ambiguous", errors

        if mcp_markers:
            return "mcp_server", []

        # Default to standard agent so existing behavior stays unchanged
        return "agent", []

    def _validate_mcp_structure(self, agent_dir: Path) -> List[str]:
        """Validate MCP-specific markers and entrypoint shape."""
        errors: List[str] = []

        entrypoint_candidates = [
            agent_dir / "src" / "main.py",
            agent_dir / "main.py",
            agent_dir / "src" / "__main__.py",
            agent_dir / "__main__.py",
        ]

        entrypoint = next((p for p in entrypoint_candidates if p.exists()), None)
        if not entrypoint:
            errors.append(
                "MCP server entrypoint not found (checked src/main.py, main.py, src/__main__.py, __main__.py)"
            )
            return errors

        try:
            content = entrypoint.read_text(encoding="utf-8", errors="ignore")
        except Exception as e:
            errors.append(f"Cannot read MCP entrypoint {entrypoint.name}: {e}")
            return errors

        mcp_entry_patterns = [
            r"\bfrom\s+fastmcp\b",
            r"\bimport\s+fastmcp\b",
            r"\bfrom\s+mcp\b",
            r"\bimport\s+mcp\b",
            r"@\s*tool\b",
            r"@\s*FastMCP\.tool\b",
            r"\bFastMCP\s*\(",
        ]

        if not any(re.search(pattern, content, re.IGNORECASE) for pattern in mcp_entry_patterns):
            errors.append(
                f"MCP artifact detected but entrypoint '{entrypoint.name}' does not contain expected MCP markers (fastmcp/mcp imports or tool decorators)"
            )

        return errors

    async def _ensure_mcp_manifest_json(
        self, agent_path: str, agent_name: str
    ) -> bool:
        """Generate MCP manifest if missing using the MCP manifest service"""
        manifest_paths = [
            Path(agent_path) / "MCPManifest.json",
            Path(agent_path) / "mcp-manifest.json",
            Path(agent_path) / "mcp_manifest.json",
        ]

        if any(path.exists() for path in manifest_paths):
            self.logger.info("MCP manifest already exists")
            return False

        self.logger.info("Generating MCP manifest using MCP manifest service")

        try:
            success = await self.mcp_manifest_service.generate_and_save_mcp_manifest(
                agent_path=agent_path,
                agent_name=agent_name,
            )

            if success:
                self.logger.info(f"Successfully generated MCP manifest for {agent_name}")
                return True

            self.logger.warning(
                f"Failed to generate MCP manifest for {agent_name}, using fallback"
            )
            return False

        except Exception as e:
            self.logger.error(
                f"Error generating MCP manifest for {agent_name}: {str(e)}"
            )
            return False

    async def process_zip_upload(
        self, file: UploadFile, agent_name: Optional[str] = None
    ) -> AgentUploadResult:
        """
        Process uploaded .zip file and create agent

        Flow:
        1. Extract zip to temp directory
        2. Validate agent structure
        3. Generate capabilities.json if missing
        4. Copy to agents directory
        5. Create registry entry
        """
        self.logger.info(f"Processing zip upload for agent: {agent_name}")

        temp_dir = None
        try:
            # Extract zip to temp directory
            temp_dir = await self._extract_zip_file(file)

            # Determine agent name if not provided
            if not agent_name:
                agent_name = await _determine_agent_name(temp_dir)

            # Validate agent structure
            validation = await self.validate_agent_structure(temp_dir)
            if not validation.is_valid:
                return AgentUploadResult(
                    success=False,
                    agent_name=agent_name,
                    status="validation_failed",
                    validation_errors=validation.errors,
                )

            # Generate metadata based on detected artifact type
            if validation.artifact_type == "mcp_server":
                await self._ensure_mcp_manifest_json(temp_dir, agent_name)
                capabilities_generated = False
            else:
                capabilities_generated = await self._ensure_agentcard_json(
                    temp_dir, agent_name
                )

            # Copy to agents directory and get the version used
            version = await self._copy_to_agents_directory(temp_dir, agent_name)

            return AgentUploadResult(
                success=True,
                agent_name=agent_name,
                status="uploaded",
                capabilities_generated=capabilities_generated,
                orchestration_triggered=False,
                version=version,
                artifact_type=validation.artifact_type,
            )

        except Exception as e:
            self.logger.error(f"Error processing zip upload: {str(e)}")
            return AgentUploadResult(
                success=False,
                agent_name=agent_name or "unknown",
                status="error",
                validation_errors=[str(e)],
            )
        finally:
            # Clean up temporary directory
            if temp_dir and os.path.exists(temp_dir):
                try:
                    shutil.rmtree(temp_dir)
                    self.logger.debug(f"Cleaned up temporary directory: {temp_dir}")
                except Exception as e:
                    self.logger.warning(
                        f"Failed to clean up temp directory {temp_dir}: {e}"
                    )

    async def process_directory_upload(
        self, directory_path: str, agent_name: Optional[str] = None
    ) -> AgentUploadResult:
        """
        Process agent upload from a local directory path (for CLI usage)

        Flow:
        1. Validate directory path and structure
        2. Generate capabilities.json if missing
        3. Copy to agents directory

        Args:
            directory_path: Path to the agent directory
            agent_name: Optional agent name (inferred from directory if not provided)

        Returns:
            AgentUploadResult with deployment details
        """
        self.logger.info(
            f"Processing directory upload for agent: {agent_name or 'auto-detect'}"
        )

        try:
            # Validate directory path
            source_dir = Path(directory_path).resolve()
            if not source_dir.exists():
                return AgentUploadResult(
                    success=False,
                    agent_name=agent_name or "unknown",
                    status="directory_not_found",
                    validation_errors=[f"Directory does not exist: {directory_path}"],
                )

            if not source_dir.is_dir():
                return AgentUploadResult(
                    success=False,
                    agent_name=agent_name or "unknown",
                    status="not_directory",
                    validation_errors=[f"Path is not a directory: {directory_path}"],
                )

            # Determine agent name if not provided (getting from docker compose, can use directory name)
            if not agent_name:
                agent_name = await _determine_agent_name(str(source_dir))
                self.logger.info(f"Determined agent name: {agent_name}")

            # Validate agent structure
            validation = await self.validate_agent_structure(str(source_dir))
            if not validation.is_valid:
                return AgentUploadResult(
                    success=False,
                    agent_name=agent_name,
                    status="validation_failed",
                    validation_errors=validation.errors,
                )

            # Generate metadata based on detected artifact type
            if validation.artifact_type == "mcp_server":
                await self._ensure_mcp_manifest_json(str(source_dir), agent_name)
                capabilities_generated = False
            else:
                capabilities_generated = await self._ensure_agentcard_json(
                    str(source_dir), agent_name
                )

            # Copy to agents directory and get the version used
            version = await self._copy_to_agents_directory(str(source_dir), agent_name)

            return AgentUploadResult(
                success=True,
                agent_name=agent_name,
                status="uploaded",
                capabilities_generated=capabilities_generated,
                orchestration_triggered=False,
                version=version,
                artifact_type=validation.artifact_type,
            )

        except Exception as e:
            self.logger.error(f"Error processing directory upload: {str(e)}")
            return AgentUploadResult(
                success=False,
                agent_name=agent_name or "unknown",
                status="error",
                validation_errors=[str(e)],
            )

    async def validate_agent_structure(self, agent_path: str) -> ValidationResult:
        """
        Validate that agent has required files and structure

        Required files:
        - Dockerfile
        - docker-compose.yml
        - src/main.py OR main.py
        """
        self.logger.info(f"Validating agent structure at: {agent_path}")

        errors = []
        agent_dir = Path(agent_path)

        # Check if directory exists
        if not agent_dir.exists() or not agent_dir.is_dir():
            errors.append(f"Invalid agent directory: {agent_path}")
            return ValidationResult(is_valid=False, errors=errors)

        # Check for Dockerfile
        dockerfile_path = agent_dir / "Dockerfile"
        if not dockerfile_path.exists():
            errors.append("Dockerfile is missing")
        else:
            # Basic Dockerfile validation
            try:
                dockerfile_content = dockerfile_path.read_text()
                if not dockerfile_content.strip():
                    errors.append("Dockerfile is empty")
                elif "FROM" not in dockerfile_content.upper():
                    errors.append("Dockerfile missing FROM instruction")
            except Exception as e:
                errors.append(f"Cannot read Dockerfile: {str(e)}")

        # Check for docker-compose.yml
        compose_path = agent_dir / "docker-compose.yml"
        if not compose_path.exists():
            errors.append("docker-compose.yml is missing")
        else:
            # Basic docker-compose validation
            try:
                compose_content = compose_path.read_text()
                if not compose_content.strip():
                    errors.append("docker-compose.yml is empty")
                else:
                    compose_data = yaml.safe_load(compose_content)
                    if (
                        not isinstance(compose_data, dict)
                        or "services" not in compose_data
                    ):
                        errors.append("docker-compose.yml missing services section")
            except yaml.YAMLError as e:
                errors.append(f"Invalid docker-compose.yml syntax: {str(e)}")
            except Exception as e:
                errors.append(f"Cannot read docker-compose.yml: {str(e)}")

        # Check for main.py entry point
        main_py_locations = [
            agent_dir / "src" / "main.py",
            agent_dir / "main.py",
            agent_dir / "src" / "__main__.py",
            agent_dir / "__main__.py",
        ]

        main_py_found = False
        for loc in main_py_locations:
            if loc.exists():
                main_py_found = True
                # Basic main.py validation
                try:
                    main_content = loc.read_text()
                    if not main_content.strip():
                        errors.append(f"main.py is empty: {loc.relative_to(agent_dir)}")
                except Exception as e:
                    errors.append(f"Cannot read main.py: {str(e)}")
                break

        if not main_py_found:
            errors.append(
                "main.py entry point not found (checked src/main.py and main.py)"
            )

        # Check for common Python files to ensure it's a valid Python project
        python_files = list(agent_dir.rglob("*.py"))
        if not python_files:
            errors.append("No Python files found in the agent directory")

        # Detect artifact type
        artifact_type, detection_errors = self._detect_artifact_type(agent_dir)
        errors.extend(detection_errors)

        # MCP-specific validation
        if artifact_type == "mcp_server":
            errors.extend(self._validate_mcp_structure(agent_dir))

        self.logger.info(
            f"Validation completed with {len(errors)} errors, artifact_type={artifact_type}"
        )

        return ValidationResult(
            is_valid=len(errors) == 0,
            errors=errors,
            artifact_type=artifact_type,
        )

    async def _extract_zip_file(self, file: UploadFile) -> str:
        """Extract uploaded zip file to temporary directory"""
        temp_dir = tempfile.mkdtemp(prefix="agent_upload_")
        self.logger.info(f"Extracting zip to: {temp_dir}")

        try:
            # Read file content
            file_content = await file.read()

            # Validate file size (limit to 100MB)
            max_size = 100 * 1024 * 1024  # 100MB
            if len(file_content) > max_size:
                raise ValueError(
                    f"File too large: {len(file_content)} bytes (max: {max_size})"
                )

            # Save uploaded file temporarily
            zip_path = os.path.join(temp_dir, "upload.zip")

            with open(zip_path, "wb") as f:
                f.write(file_content)

            # Validate zip file
            if not zipfile.is_zipfile(zip_path):
                raise ValueError("Invalid zip file")

            # Extract zip file with security checks
            with zipfile.ZipFile(zip_path, "r") as zip_ref:
                # Check for zip bomb (too many files)
                if len(zip_ref.namelist()) > 1000:
                    raise ValueError("Zip file contains too many files (max: 1000)")

                # Check for directory traversal attacks
                for member in zip_ref.namelist():
                    if os.path.isabs(member) or ".." in member:
                        raise ValueError(f"Unsafe path in zip: {member}")

                # Extract all files
                zip_ref.extractall(temp_dir)
                self.logger.info(f"Extracted {len(zip_ref.namelist())} files")

            # Remove the zip file, keep extracted contents
            os.remove(zip_path)

            # Find the actual agent directory (in case files are in a subdirectory)
            extracted_dir = self._find_agent_directory(temp_dir)

            return extracted_dir

        except Exception as e:
            # Clean up on error
            if os.path.exists(temp_dir):
                shutil.rmtree(temp_dir)
            raise ValueError(f"Failed to extract zip file: {str(e)}")

    def _find_agent_directory(self, temp_dir: str) -> str:
        """Find the actual agent directory within extracted content"""
        temp_path = Path(temp_dir)

        # List all items in temp directory
        items = list(temp_path.iterdir())

        # Filter out the uploaded zip file if it still exists
        items = [item for item in items if item.name != "upload.zip"]

        # If there's only one directory, check if it contains agent files
        if len(items) == 1 and items[0].is_dir():
            potential_agent_dir = items[0]
            # Check if this directory contains Dockerfile or docker-compose.yml
            if (potential_agent_dir / "Dockerfile").exists() or (
                potential_agent_dir / "docker-compose.yml"
            ).exists():
                self.logger.info(f"Found agent directory: {potential_agent_dir}")
                return str(potential_agent_dir)

        # Otherwise, assume the temp directory itself contains the agent files
        if (temp_path / "Dockerfile").exists() or (
            temp_path / "docker-compose.yml"
        ).exists():
            self.logger.info(f"Agent files found in root: {temp_dir}")
            return temp_dir

        # If we have multiple directories, try to find one with agent files
        for item in items:
            if item.is_dir():
                if (item / "Dockerfile").exists() or (
                    item / "docker-compose.yml"
                ).exists():
                    self.logger.info(f"Found agent directory: {item}")
                    return str(item)

        # Default to temp directory
        self.logger.warning(f"Could not identify agent directory, using: {temp_dir}")
        return temp_dir

    async def _ensure_agentcard_json(
        self, agent_path: str, agent_name: str, n8n_agent: bool = False
    ) -> bool:
        """Generate AgentCard.json if missing using agentcard service"""
        agentcard_path = Path(agent_path) / "AgentCard.json"

        if agentcard_path.exists():
            self.logger.info("AgentCard.json already exists")
            return False

        # Generate AgentCard.json using the agentcard service
        self.logger.info("Generating AgentCard.json using agentcard service")

        try:
            # Use the agentcard service to generate AgentCard
            success = await self.agentcard_service.generate_and_save_agentcard(
                agent_path=agent_path,
                agent_name=agent_name,
                n8n_agent=n8n_agent,
                base_url=settings.NASIKO_API_URL,
            )

            if success:
                self.logger.info(
                    f"Successfully generated AgentCard.json for {agent_name}"
                )
                return True
            else:
                self.logger.warning(
                    f"Failed to generate AgentCard for {agent_name}, using fallback"
                )
                return False

        except Exception as e:
            self.logger.error(f"Error generating AgentCard for {agent_name}: {str(e)}")
            return False

    async def _get_version_from_agentcard(self, agent_path: str) -> str:
        """Get version from AgentCard.json, fallback to v1.0.0 if not found"""
        try:
            agentcard = await self.agentcard_service.load_agentcard_from_file(
                agent_path
            )
            if agentcard and "version" in agentcard:
                version = agentcard["version"]
                # Ensure version has 'v' prefix for directory naming
                if not version.startswith("v"):
                    version = f"v{version}"
                self.logger.info(f"Found version {version} in AgentCard.json")
                return version
            else:
                self.logger.warning(
                    "No version found in AgentCard.json, using default v1.0.0"
                )
                return "v1.0.0"
        except Exception as e:
            self.logger.warning(
                f"Failed to read version from AgentCard: {e}, using default v1.0.0"
            )
            return "v1.0.0"

    async def _copy_to_agents_directory(self, temp_dir: str, agent_name: str):
        """Copy agent from temp directory to agents/{agent_name}/{version}/"""
        # Get version from AgentCard.json (will fallback to v1.0.0 if not found)
        version = await self._get_version_from_agentcard(temp_dir)

        # Create versioned directory for initial upload
        agent_base_dir = self.agents_directory / agent_name
        target_dir = agent_base_dir / version

        # Ensure base directory exists
        agent_base_dir.mkdir(parents=True, exist_ok=True)

        # Handle existing agent version
        if target_dir.exists():
            self.logger.warning(
                f"Agent {agent_name} {version} already exists, overwriting"
            )
            shutil.rmtree(target_dir)

        shutil.copytree(temp_dir, target_dir)
        self.logger.info(f"Copied agent to: {target_dir}")

        # Return the version used for directory naming
        return version

    def __del__(self):
        """Cleanup any temporary directories"""
        # TODO: Add cleanup logic for temp directories
        pass
