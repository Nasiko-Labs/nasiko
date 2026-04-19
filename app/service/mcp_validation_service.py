"""
MCP Server Validation Service
Validates MCP server structure and required components.
"""

import yaml
from pathlib import Path
from typing import List, Optional


class MCPValidationResult:
    """Result of MCP server validation"""
    
    def __init__(self, is_valid: bool, errors: List[str] = None):
        self.is_valid = is_valid
        self.errors = errors or []


class MCPValidationService:
    """Validates MCP server structure"""
    
    def __init__(self, logger=None):
        self.logger = logger
    
    async def validate_mcp_structure(self, server_path: str) -> MCPValidationResult:
        """
        Validate that MCP server has required files and structure.
        
        Required files:
        - Dockerfile (with MCP base or FROM handler)
        - docker-compose.yml (with service definition)
        - src/main.py (entry point with MCP decorators)
        
        Args:
            server_path: Path to the MCP server directory
            
        Returns:
            MCPValidationResult with validation status and errors
        """
        if self.logger:
            self.logger.info(f"Validating MCP server structure at: {server_path}")
        
        errors = []
        server_dir = Path(server_path)
        
        # Check if directory exists
        if not server_dir.exists() or not server_dir.is_dir():
            errors.append(f"Invalid MCP server directory: {server_path}")
            return MCPValidationResult(is_valid=False, errors=errors)
        
        # Check for Dockerfile
        dockerfile_path = server_dir / "Dockerfile"
        if not dockerfile_path.exists():
            errors.append("Dockerfile is missing (required for MCP server)")
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
        compose_path = server_dir / "docker-compose.yml"
        if not compose_path.exists():
            errors.append("docker-compose.yml is missing (required for MCP server)")
        else:
            # Validate docker-compose structure
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
            server_dir / "src" / "main.py",
            server_dir / "main.py",
            server_dir / "src" / "__main__.py",
            server_dir / "__main__.py",
        ]
        
        main_py_found = False
        for loc in main_py_locations:
            if loc.exists():
                main_py_found = True
                # Basic main.py validation
                try:
                    main_content = loc.read_text()
                    if not main_content.strip():
                        errors.append(f"main.py is empty: {loc.relative_to(server_dir)}")
                    else:
                        # Check for MCP decorators or imports
                        if not ("@mcp." in main_content or "from mcp" in main_content):
                            errors.append(
                                f"main.py missing MCP decorators/imports: {loc.relative_to(server_dir)}"
                            )
                except Exception as e:
                    errors.append(f"Cannot read main.py: {str(e)}")
                break
        
        if not main_py_found:
            errors.append(
                "main.py entry point not found (checked src/main.py and main.py)"
            )
        
        # Check for Python files to ensure it's a valid Python project
        python_files = list(server_dir.rglob("*.py"))
        if not python_files:
            errors.append("No Python files found in the MCP server directory")
        
        if self.logger:
            self.logger.info(f"MCP validation completed with {len(errors)} errors")
        
        return MCPValidationResult(is_valid=len(errors) == 0, errors=errors)
