"""
MCP Server Manifest Generator
Generates McpServerManifest.json for uploaded MCP servers.

Generation strategy:
1. Try LLM-driven generator that reuses app/utils/agentcard_generator tool loop.
2. Fall back to local AST parsing when no model key is configured or LLM generation fails.
"""

import ast
import asyncio
import json
import os
from pathlib import Path
from typing import Dict, List, Any, Optional


class MCPManifestGenerator:
    """Generates MCP server manifests with LLM-first and AST fallback."""
    
    def __init__(self, logger=None):
        self.logger = logger
    
    @staticmethod
    def _find_python_entry_point(server_path: str) -> Optional[Path]:
        """Find the main.py entry point"""
        server_dir = Path(server_path)
        
        locations = [
            server_dir / "src" / "main.py",
            server_dir / "main.py",
            server_dir / "src" / "__main__.py",
            server_dir / "__main__.py",
        ]
        
        for loc in locations:
            if loc.exists() and loc.is_file():
                return loc
        
        return None
    
    @staticmethod
    def _extract_string_value(node: ast.expr) -> Optional[str]:
        """
        Extract string value from AST node.
        Handles ast.Constant (Python 3.8+) and ast.Str (older versions).
        """
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return node.value
        if isinstance(node, ast.Str):
            return node.s
        return None
    
    @staticmethod
    def _extract_dict_value(node: ast.expr) -> Optional[Dict]:
        """Try to extract dict structure from AST node"""
        if isinstance(node, ast.Dict):
            result = {}
            for key_node, val_node in zip(node.keys, node.values):
                key = None
                if isinstance(key_node, ast.Constant):
                    key = key_node.value
                elif isinstance(key_node, ast.Str):
                    key = key_node.s
                
                if key:
                    # Try to extract value
                    if isinstance(val_node, ast.Constant):
                        result[key] = val_node.value
                    elif isinstance(val_node, ast.Str):
                        result[key] = val_node.s
                    elif isinstance(val_node, ast.Dict):
                        result[key] = MCPManifestGenerator._extract_dict_value(val_node)
            
            return result if result else None
        
        return None
    
    def _parse_mcp_decorators(self, python_content: str) -> Dict[str, List[Dict]]:
        """
        Parse Python file and extract MCP decorators and lower-level registrations.
        
        Returns:
            Dict with 'tools', 'resources', 'prompts' keys
        """
        try:
            tree = ast.parse(python_content)
        except SyntaxError as e:
            if self.logger:
                self.logger.error(f"Syntax error parsing main.py: {e}")
            return {"tools": [], "resources": [], "prompts": []}
        
        tools: List[Dict] = []
        resources: List[Dict] = []
        prompts: List[Dict] = []
        seen = {"tools": set(), "resources": set(), "prompts": set()}

        def _add_unique(target: str, item: Dict[str, Any]):
            key = item.get("name") or ""
            if not key:
                return
            if key in seen[target]:
                return
            seen[target].add(key)
            if target == "tools":
                tools.append(item)
            elif target == "resources":
                resources.append(item)
            else:
                prompts.append(item)

        def _extract_registration_name(call_node: ast.Call, fallback: str) -> str:
            # Prefer first positional string argument.
            if call_node.args:
                first = self._extract_string_value(call_node.args[0])
                if first:
                    return first

            # Then check common keyword names.
            for keyword in call_node.keywords:
                if keyword.arg in {
                    "name",
                    "tool",
                    "resource",
                    "prompt",
                    "id",
                    "path",
                }:
                    kw_val = self._extract_string_value(keyword.value)
                    if kw_val:
                        return kw_val

            return fallback
        
        decorator_call_ids = set()

        for node in ast.walk(tree):
            # Look for function definitions with decorators
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                for decorator in node.decorator_list:
                    decorator_name = None
                    decorator_args = []
                    decorator_kwargs = {}

                    if isinstance(decorator, ast.Call):
                        decorator_call_ids.add(id(decorator))
                    
                    # Handle @decorator.method or @decorator
                    if isinstance(decorator, ast.Attribute):
                        if isinstance(decorator.value, ast.Name):
                            if decorator.value.id == "mcp":
                                decorator_name = decorator.attr
                    elif isinstance(decorator, ast.Call):
                        # Handle @decorator(...) calls
                        if isinstance(decorator.func, ast.Attribute):
                            if isinstance(decorator.func.value, ast.Name):
                                if decorator.func.value.id == "mcp":
                                    decorator_name = decorator.func.attr
                                    # Extract decorator arguments
                                    for arg in decorator.args:
                                        if isinstance(arg, ast.Constant):
                                            decorator_args.append(arg.value)
                                        elif isinstance(arg, ast.Str):
                                            decorator_args.append(arg.s)
                                    
                                    # Extract keyword arguments
                                    for keyword in decorator.keywords:
                                        if isinstance(keyword.value, ast.Constant):
                                            decorator_kwargs[keyword.arg] = keyword.value.value
                                        elif isinstance(keyword.value, ast.Str):
                                            decorator_kwargs[keyword.arg] = keyword.value.s
                    
                    if decorator_name in {"tool", "resource", "prompt"}:
                        # Extract function info
                        item_name = (
                            decorator_kwargs.get("name")
                            or (decorator_args[0] if decorator_args else node.name)
                            or node.name
                        )
                        func_info = {
                            "name": item_name,
                            "description": ast.get_docstring(node) or "",
                            "parameters": self._extract_function_parameters(node),
                        }
                        
                        # Add decorator-specific kwargs
                        func_info.update(decorator_kwargs)
                        
                        if decorator_name == "tool":
                            _add_unique("tools", func_info)
                        elif decorator_name == "resource":
                            _add_unique("resources", func_info)
                        elif decorator_name == "prompt":
                            _add_unique("prompts", func_info)

            # Also detect lower-level explicit registration calls.
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute):
                # Ignore decorator invocations already processed above.
                if id(node) in decorator_call_ids:
                    continue

                attr = node.func.attr
                registration_mapping = {
                    "tool": "tools",
                    "add_tool": "tools",
                    "register_tool": "tools",
                    "resource": "resources",
                    "add_resource": "resources",
                    "register_resource": "resources",
                    "prompt": "prompts",
                    "add_prompt": "prompts",
                    "register_prompt": "prompts",
                }

                target = registration_mapping.get(attr)
                if target:
                    default_name = f"{attr}_{getattr(node, 'lineno', 0)}"
                    entry = {
                        "name": _extract_registration_name(node, default_name),
                        "description": "",
                        "parameters": {},
                    }

                    for keyword in node.keywords:
                        if keyword.arg == "description":
                            description = self._extract_string_value(keyword.value)
                            if description:
                                entry["description"] = description
                        elif keyword.arg == "parameters":
                            params = self._extract_dict_value(keyword.value)
                            if params:
                                entry["parameters"] = params

                    _add_unique(target, entry)
        
        return {
            "tools": tools,
            "resources": resources,
            "prompts": prompts,
        }
    
    @staticmethod
    def _extract_function_parameters(
        func_node: ast.FunctionDef | ast.AsyncFunctionDef,
    ) -> Dict[str, Any]:
        """Extract parameter information from function signature"""
        parameters = {}
        
        for arg in func_node.args.args:
            # Skip 'self' parameter
            if arg.arg == "self":
                continue
            
            param_info = {"type": "string"}
            
            # Try to extract type annotation
            if arg.annotation:
                if isinstance(arg.annotation, ast.Name):
                    param_info["type"] = arg.annotation.id
                elif isinstance(arg.annotation, ast.Constant):
                    param_info["type"] = str(arg.annotation.value)
            
            parameters[arg.arg] = param_info
        
        return parameters
    
    async def generate_manifest(
        self, server_path: str, server_name: str
    ) -> Optional[Dict[str, Any]]:
        """
        Generate MCP server manifest by parsing main.py.
        
        Args:
            server_path: Path to the MCP server directory
            server_name: Name of the MCP server
            
        Returns:
            Dict with manifest structure, or None if generation fails
        """
        if self.logger:
            self.logger.info(f"Generating MCP manifest for: {server_name}")

        # Preferred path: shared tool-loop LLM generator.
        llm_manifest = await self._generate_manifest_with_llm(server_path, server_name)
        if self._is_manifest_usable(llm_manifest):
            return llm_manifest
        
        # Find entry point
        main_py_path = self._find_python_entry_point(server_path)
        if not main_py_path:
            if self.logger:
                self.logger.error(f"No main.py found at {server_path}")
            return None
        
        # Read Python file
        try:
            python_content = main_py_path.read_text()
        except Exception as e:
            if self.logger:
                self.logger.error(f"Failed to read main.py: {e}")
            return None
        
        # Parse decorators
        mcp_components = self._parse_mcp_decorators(python_content)
        
        # Build manifest
        manifest = {
            "name": server_name,
            "version": "1.0.0",
            "tools": mcp_components["tools"],
            "resources": mcp_components["resources"],
            "prompts": mcp_components["prompts"],
        }
        
        if self.logger:
            self.logger.info(
                f"Generated manifest with {len(mcp_components['tools'])} tools, "
                f"{len(mcp_components['resources'])} resources, "
                f"{len(mcp_components['prompts'])} prompts"
            )
        
        return manifest

    async def _generate_manifest_with_llm(
        self, server_path: str, server_name: str
    ) -> Optional[Dict[str, Any]]:
        """Try generating manifest through shared LLM loop scaffolding."""
        api_key = os.getenv("OPENAI_API_KEY") or os.getenv("MINIMAX_API_KEY")
        if not api_key:
            return None

        try:
            from app.utils.agentcard_generator import MCPManifestGeneratorAgent

            generator = MCPManifestGeneratorAgent(
                api_key=api_key,
                model=os.getenv("MCP_MANIFEST_MODEL", "gpt-4o-mini"),
            )
            result = await asyncio.to_thread(
                generator.generate_manifest,
                server_path,
                server_name,
                False,
            )
            if result.get("status") == "success":
                return result.get("manifest")

            if self.logger:
                self.logger.warning(
                    f"LLM MCP manifest generation did not succeed: {result.get('message')}"
                )
            return None
        except Exception as e:
            if self.logger:
                self.logger.warning(
                    f"LLM MCP manifest generation failed, falling back to AST parser: {e}"
                )
            return None

    @staticmethod
    def _is_manifest_usable(manifest: Optional[Dict[str, Any]]) -> bool:
        """Check whether a generated manifest has enough structure to use."""
        if not manifest or not isinstance(manifest, dict):
            return False

        if not manifest.get("name"):
            return False

        for field in ("tools", "resources", "prompts"):
            value = manifest.get(field, [])
            if isinstance(value, list) and len(value) > 0:
                return True

        return False
    
    async def save_manifest(
        self, manifest: Dict[str, Any], output_path: str
    ) -> bool:
        """
        Save manifest to McpServerManifest.json.
        
        Args:
            manifest: The manifest dict to save
            output_path: Path to save to (directory or full path)
            
        Returns:
            True if saved successfully, False otherwise
        """
        try:
            output_path_obj = Path(output_path)
            
            # If directory, append filename
            if output_path_obj.is_dir() or not output_path_obj.suffix:
                output_path_obj = output_path_obj / "McpServerManifest.json"
            
            # Ensure parent directory exists
            output_path_obj.parent.mkdir(parents=True, exist_ok=True)
            
            # Write manifest
            with open(output_path_obj, "w", encoding="utf-8") as f:
                json.dump(manifest, f, indent=2)
            
            if self.logger:
                self.logger.info(f"Manifest saved to: {output_path_obj}")
            
            return True
        
        except Exception as e:
            if self.logger:
                self.logger.error(f"Failed to save manifest: {e}")
            return False
