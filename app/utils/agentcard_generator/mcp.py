import ast
import json
import logging
from pathlib import Path
from typing import Dict, Any, List

logger = logging.getLogger(__name__)

class MCPManifestGenerator:
    """
    Statically analyzes Python code utilizing FastMCP (or standard MCP) to extract tool definitions
    and generate an mcp_manifest.json equivalent. This bypasses LLM hallucinations for strict API definitions.
    """
    def __init__(self):
        pass

    def extract_capabilities(self, file_path: str) -> Dict[str, Any]:
        """Parse AST to detect MCP tools and generate manifest JSON."""
        try:
            with open(file_path, "r", encoding="utf-8") as f:
                source = f.read()
            tree = ast.parse(source)
        except Exception as e:
            logger.error(f"Failed to parse python AST for {file_path}: {e}")
            return {"status": "error", "message": str(e)}

        tools = []
        is_mcp_server = False
        server_name = "mcp_server"

        for node in ast.walk(tree):
            # Detect server instantiation (mcp = FastMCP("name"))
            if isinstance(node, ast.Assign):
                for target in node.targets:
                    if isinstance(node.value, ast.Call) and getattr(node.value.func, "id", "") == "FastMCP":
                        is_mcp_server = True
                        if node.value.args and isinstance(node.value.args[0], ast.Constant):
                            server_name = node.value.args[0].value

            # Detect @mcp.tool() decorators
            if isinstance(node, ast.FunctionDef):
                for decorator in node.decorator_list:
                    # check for @mcp.tool() or @app.tool() etc
                    if isinstance(decorator, ast.Call) and getattr(decorator.func, "attr", "") == "tool":
                        is_mcp_server = True
                        tools.append(self._parse_mcp_tool(node))
                    elif getattr(decorator, "attr", "") == "tool":
                        is_mcp_server = True
                        tools.append(self._parse_mcp_tool(node))

        if not is_mcp_server:
            # Maybe standard MCP import is used
            if "mcp" in source:
                is_mcp_server = True

        if not is_mcp_server:
            return {"status": "error", "message": "No MCP or FastMCP Server detected in code."}

        manifest = {
            "name": server_name,
            "version": "1.0.0",
            "artifactType": "mcp_server",
            "transport": "stdio",  # Defaulting to stdio for hackathon
            "tools": tools
        }

        return {"status": "success", "manifest": manifest}

    def _parse_mcp_tool(self, node: ast.FunctionDef) -> Dict[str, Any]:
        """Extracts tool metadata from an AST function definition."""
        properties = {}
        required = []

        # Parse arguments for JSON schema
        for arg in node.args.args:
            if arg.arg == "self" or arg.arg == "ctx" or arg.arg == "context":
                continue
            required.append(arg.arg)
            prop_type = "string" # default
            if arg.annotation:
                if isinstance(arg.annotation, ast.Name):
                    if arg.annotation.id == "int": prop_type = "integer"
                    elif arg.annotation.id == "float": prop_type = "number"
                    elif arg.annotation.id == "bool": prop_type = "boolean"
                    elif arg.annotation.id == "list": prop_type = "array"
                    elif arg.annotation.id == "dict": prop_type = "object"
            properties[arg.arg] = {"type": prop_type}

        docstring = ast.get_docstring(node)
        
        return {
            "name": node.name,
            "description": docstring or f"Auto-generated tool for {node.name}",
            "inputSchema": {
                "type": "object",
                "properties": properties,
                "required": required
            }
        }
