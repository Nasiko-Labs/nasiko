"""
AgentCard Service

Service layer for generating A2A-compliant AgentCard.json files
using LLM-based analysis.
"""

import json
import os
import ast
import re
from pathlib import Path
from typing import Optional, Dict, Any

from app.utils.agentcard_generator import AgentCardGeneratorAgent
from app.utils.agentcard_generator.mcp_manifest_generator import McpManifestGeneratorAgent


class AgentCardService:
    """
    Service for generating and managing A2A-compliant AgentCards
    """

    def __init__(self, logger, openai_api_key: Optional[str] = None):
        self.logger = logger
        self.openai_api_key = (
            openai_api_key
            or os.getenv("OPENAI_API_KEY")
            or os.getenv("MINIMAX_API_KEY")
        )

    async def generate_and_save_agentcard(
        self,
        agent_path: str,
        agent_name: str,
        n8n_agent: bool,
        base_url: str = "http://localhost:8000",
    ) -> bool:
        """
        Generate AgentCard.json file for an agent and save it to the agent directory

        Args:
            agent_path: Path to the agent directory
            agent_name: Name of the agent
            base_url: Base URL for the agent service
            n8n_agent: whether to generate n8n registry data
        Returns:
            True if AgentCard was generated successfully, False otherwise

        """

        try:
            self.logger.info(f"Generating AgentCard for {agent_name} at {agent_path}")

            # Initialize the AgentCard generator agent
            generator = AgentCardGeneratorAgent(
                api_key=self.openai_api_key,
                model="gpt-4o",
                n8n_agent=n8n_agent,
            )

            # Generate AgentCard using the agent
            result = generator.generate_agentcard(agent_path=agent_path, verbose=False)

            if result["status"] != "success" or not result.get("agentcard"):
                self.logger.error(
                    f"Failed to generate AgentCard: {result.get('message')}"
                )
                return False

            agentcard = result["agentcard"]

            # Save AgentCard.json to the agent directory
            agentcard_path = Path(agent_path) / "AgentCard.json"
            with open(agentcard_path, "w") as f:
                json.dump(agentcard, f, indent=2, ensure_ascii=False)

            self.logger.info(f"Successfully saved AgentCard.json to {agentcard_path}")
            return True

        except Exception as e:
            self.logger.error(
                f"Failed to generate AgentCard for {agent_name}: {str(e)}"
            )
            return False

    async def generate_and_save_mcp_manifest(
        self,
        agent_path: str,
        agent_name: str,
        base_url: str = "http://localhost:8000",
    ) -> bool:
        """
        Generate McpServerManifest.json file for an MCP server and save it
        """
        try:
            self.logger.info(f"Generating McpServerManifest for {agent_name} at {agent_path}")

            # Initialize the MCP Manifest generator agent
            generator = McpManifestGeneratorAgent(
                api_key=self.openai_api_key,
                model="gpt-4o",
            )

            # Generate manifest
            result = generator.generate_mcp_manifest(agent_path=agent_path, verbose=False)

            if result["status"] != "success" or not result.get("agentcard"): # uses 'agentcard' dict internally
                self.logger.error(
                    f"Failed to generate MCP Manifest: {result.get('message')}"
                )
                return self._generate_and_save_mcp_manifest_fallback(agent_path, agent_name)

            manifest = result["agentcard"]

            # Save McpServerManifest.json
            manifest_path = Path(agent_path) / "McpServerManifest.json"
            with open(manifest_path, "w") as f:
                json.dump(manifest, f, indent=2, ensure_ascii=False)

            self.logger.info(f"Successfully saved McpServerManifest.json to {manifest_path}")
            return True

        except Exception as e:
            self.logger.error(
                f"Failed to generate MCP Manifest for {agent_name}: {str(e)}"
            )
            return self._generate_and_save_mcp_manifest_fallback(agent_path, agent_name)

    def _generate_and_save_mcp_manifest_fallback(
        self, agent_path: str, agent_name: str
    ) -> bool:
        """Create a deterministic MCP manifest from decorators when LLM generation is unavailable."""
        try:
            root = Path(agent_path)
            manifest = self.build_mcp_manifest_fallback(agent_path, agent_name)

            manifest_path = root / "McpServerManifest.json"
            with open(manifest_path, "w") as f:
                json.dump(manifest, f, indent=2, ensure_ascii=False)

            self.logger.info(
                f"Saved fallback McpServerManifest.json to {manifest_path} with {len(manifest.get('tools', []))} tools"
            )
            return True
        except Exception as e:
            self.logger.error(f"Fallback MCP manifest generation failed: {e}")
            return False

    async def generate_mcp_manifest_from_url(
        self,
        url: str,
        name: str,
    ) -> Optional[Dict[str, Any]]:
        """
        Generate an McpServerManifest from a remote HTTP MCP server URL.
        """
        import requests
        try:
            self.logger.info(f"Fetching tools from remote MCP server: {url}")
            # Standard MCP HTTP path for tools is /tools
            resp = requests.get(f"{url}/tools", timeout=10)
            if resp.status_code != 200:
                self.logger.error(f"Failed to fetch tools from {url}: {resp.status_code}")
                return None
            
            tools_data = resp.json().get("tools", [])
            return self.build_mcp_manifest_from_tools_data(tools_data, name, url)
        except Exception as e:
            self.logger.error(f"Error generating manifest from URL {url}: {e}")
            return None

    def build_mcp_manifest_from_tools_data(
        self, tools_data: List[Dict[str, Any]], name: str, url: str
    ) -> Dict[str, Any]:
        """Build an McpServerManifest from tool list data."""
        return {
            "id": name,
            "name": name,
            "description": f"Remote MCP server registered at: {url}",
            "version": "1.0.0",
            "artifact_type": "remote_mcp",
            "transport": "http",
            "url": url,
            "bridge": {
                "type": "none", # It's already HTTP
                "endpoints": {
                    "health": "/health",
                    "list_tools": "/tools",
                    "call_tool": "/tools/call",
                    "list_resources": "/resources",
                },
            },
            "tools": tools_data,
            "resources": [],
            "prompts": [],
        }

    def build_mcp_manifest_fallback(
        self, agent_path: str, agent_name: str
    ) -> Dict[str, Any]:
        """Build a deterministic MCP manifest from decorators without writing files."""
        root = Path(agent_path)
        tools = []
        resources = []
        prompts = []
        server_name = agent_name

        for py_file in root.rglob("*.py"):
            try:
                source = py_file.read_text(encoding="utf-8")
                tree = ast.parse(source)
            except Exception:
                continue

            name_match = re.search(r"FastMCP\(\s*['\"]([^'\"]+)['\"]", source)
            if name_match:
                server_name = name_match.group(1)

            for node in ast.walk(tree):
                if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    continue

                decorator_names = [self._decorator_name(d) for d in node.decorator_list]
                if any(name.endswith(".tool") or name == "tool" for name in decorator_names):
                    tools.append(self._function_schema(node))
                elif any(name.endswith(".resource") or name == "resource" for name in decorator_names):
                    resources.append(
                        {
                            "name": node.name,
                            "description": ast.get_docstring(node) or f"MCP resource {node.name}",
                        }
                    )
                elif any(name.endswith(".prompt") or name == "prompt" for name in decorator_names):
                    prompts.append(
                        {
                            "name": node.name,
                            "description": ast.get_docstring(node) or f"MCP prompt {node.name}",
                        }
                    )

        return {
            "id": agent_name,
            "name": server_name,
            "description": f"MCP server published through Nasiko: {server_name}",
            "version": "1.0.0",
            "artifact_type": "mcp_server",
            "transport": "stdio",
            "bridge": {
                "type": "http",
                "endpoints": {
                    "health": "/health",
                    "list_tools": "/tools",
                    "call_tool": "/tools/call",
                    "list_resources": "/resources",
                },
            },
            "tools": tools,
            "resources": resources,
            "prompts": prompts,
        }

    def _decorator_name(self, decorator: ast.AST) -> str:
        if isinstance(decorator, ast.Call):
            return self._decorator_name(decorator.func)
        if isinstance(decorator, ast.Attribute):
            base = self._decorator_name(decorator.value)
            return f"{base}.{decorator.attr}" if base else decorator.attr
        if isinstance(decorator, ast.Name):
            return decorator.id
        return ""

    def _function_schema(self, node: ast.FunctionDef | ast.AsyncFunctionDef) -> Dict[str, Any]:
        properties: Dict[str, Any] = {}
        required = []
        defaults_by_arg = {
            arg.arg: default
            for arg, default in zip(
                node.args.args[-len(node.args.defaults) :] if node.args.defaults else [],
                node.args.defaults,
            )
        }

        for arg in node.args.args:
            if arg.arg in {"self", "cls"}:
                continue
            properties[arg.arg] = {
                "type": self._annotation_to_json_type(arg.annotation),
                "description": f"Argument {arg.arg}",
            }
            if arg.arg not in defaults_by_arg:
                required.append(arg.arg)

        return {
            "name": node.name,
            "description": ast.get_docstring(node) or f"MCP tool {node.name}",
            "input_schema": {
                "type": "object",
                "properties": properties,
                "required": required,
            },
        }

    def _annotation_to_json_type(self, annotation: ast.AST | None) -> str:
        if annotation is None:
            return "string"
        name = ""
        if isinstance(annotation, ast.Name):
            name = annotation.id
        elif isinstance(annotation, ast.Constant):
            name = str(annotation.value)
        elif isinstance(annotation, ast.Subscript):
            name = self._decorator_name(annotation.value)
        elif isinstance(annotation, ast.Attribute):
            name = self._decorator_name(annotation)

        return {
            "str": "string",
            "int": "integer",
            "float": "number",
            "bool": "boolean",
            "dict": "object",
            "Dict": "object",
            "list": "array",
            "List": "array",
        }.get(name, "string")

    async def load_agentcard_from_file(
        self, agent_path: str, filename: str = "AgentCard.json"
    ) -> Optional[Dict[str, Any]]:
        """
        Load AgentCard.json (or manifest) from an agent directory

        Args:
            agent_path: Path to the agent directory
            filename: File to load (defaults to AgentCard.json)
        """

        try:
            agentcard_path = Path(agent_path) / filename

            if not agentcard_path.exists():
                self.logger.warning(f"{filename} not found at {agentcard_path}")
                return None

            with open(agentcard_path, "r") as f:
                agentcard = json.load(f)

            self.logger.info(f"Successfully loaded from {agentcard_path}")
            return agentcard

        except Exception as e:
            self.logger.error(f"Failed to load AgentCard from {agent_path}: {str(e)}")
            return None

    async def generate_registry_data(
        self,
        agent_path: str,
        agent_name: str,
        url: str,
        base_url: str = "http://localhost:8000",
        n8n_agent: bool = False,
    ) -> Dict[str, Any]:
        """
        Generate registry data for an agent from AgentCard

        Args:
            agent_path: Path to the agent directory
            agent_name: Name of the agent
            url: URL where the agent is deployed
            base_url: Base URL for the agent service
            n8n_agent: whether to generate n8n registry data

        Returns:
            Dict in registry format ready for database insertion
        """

        try:
            # First try to load existing AgentCard.json
            agentcard = await self.load_agentcard_from_file(agent_path)

            if not agentcard:
                # Generate new AgentCard if file doesn't exist
                self.logger.info(
                    f"No existing AgentCard found, generating new one for {agent_name}"
                )
                await self.generate_and_save_agentcard(
                    agent_path, agent_name, n8n_agent, base_url
                )
                agentcard = await self.load_agentcard_from_file(agent_path)

            if not agentcard:
                # Fallback if generation failed
                self.logger.warning(
                    f"Failed to generate AgentCard for {agent_name}, using minimal registry data"
                )
                return self._create_minimal_registry_data(agent_name, url)

            # Convert AgentCard to registry format
            registry_data = self._convert_to_registry_format(agentcard, url)

            self.logger.info(f"Successfully generated registry data for {agent_name}")
            return registry_data

        except Exception as e:
            self.logger.error(
                f"Failed to generate registry data for {agent_name}: {str(e)}"
            )
            return self._create_minimal_registry_data(agent_name, url)

    def _create_minimal_registry_data(
        self, agent_name: str, url: str
    ) -> Dict[str, Any]:
        """
        Create minimal registry data when AgentCard generation fails

        Args:
            agent_name: Name of the agent
            url: URL where the agent is deployed

        Returns:
            Minimal registry data following A2A schema
        """

        return {
            "name": agent_name,
            "description": f"Auto-uploaded agent: {agent_name}",
            "url": url,
            "version": "1.0.0",
            "capabilities": {
                "streaming": False,
                "pushNotifications": False,
                "stateTransitionHistory": False,
            },
            "defaultInputModes": ["application/json", "text/plain"],
            "defaultOutputModes": ["application/json"],
            "skills": [],
        }

    async def validate_agentcard_file(self, agent_path: str) -> bool:
        """
        Validate that AgentCard.json exists and has proper structure

        Args:
            agent_path: Path to the agent directory

        Returns:
            True if AgentCard file is valid, False otherwise
        """

        try:
            agentcard = await self.load_agentcard_from_file(agent_path)

            if not agentcard:
                return False

            # Basic validation for A2A AgentCard
            required_keys = [
                "name",
                "description",
                "url",
                "version",
                "capabilities",
                "skills",
            ]
            for key in required_keys:
                if key not in agentcard:
                    self.logger.error(f"Missing required key in AgentCard: {key}")
                    return False

            # Validate capabilities section
            capabilities = agentcard.get("capabilities", {})
            capability_keys = [
                "streaming",
                "pushNotifications",
                "stateTransitionHistory",
            ]

            for key in capability_keys:
                if key not in capabilities:
                    self.logger.warning(f"Missing capability key in AgentCard: {key}")

            self.logger.info("AgentCard file validation passed")
            return True

        except Exception as e:
            self.logger.error(f"Error validating AgentCard file: {str(e)}")
            return False

    def _convert_to_registry_format(
        self, agentcard: Dict[str, Any], url: str
    ) -> Dict[str, Any]:
        """
        Convert AgentCard.json format to registry database format

        Args:
            agentcard: Dict containing the A2A AgentCard structure
            url: URL where the agent is deployed

        Returns:
            Dict in registry format for database storage
        """

        # Update the url field for registry
        registry_data = agentcard.copy()
        registry_data["url"] = url

        self.logger.info(
            f"Converted AgentCard for {agentcard.get('name', 'unknown')} with {len(agentcard.get('skills', []))} skills"
        )

        return registry_data
