import json
import logging
import os
import sys
from pathlib import Path
from typing import Any, Dict, List

from openai import OpenAI
from app.utils.agentcard_generator.tools import AgentAnalyzerTools

logger = logging.getLogger(__name__)

class McpManifestGeneratorAgent:
    """
    An agent that generates MCP Server Manifests by analyzing the MCP server code
    """

    def __init__(
        self,
        api_key: str = None,
        model: str = "gpt-4o",
        base_url: str = None,
    ):
        self.api_key = (
            api_key or os.getenv("OPENAI_API_KEY") or os.getenv("MINIMAX_API_KEY")
        )
        if not self.api_key:
            logger.error("No API key found in environment or arguments")
            raise ValueError("OPENAI_API_KEY or MINIMAX_API_KEY must be set")

        if (
            not base_url
            and not api_key
            and not os.getenv("OPENAI_API_KEY")
            and os.getenv("MINIMAX_API_KEY")
        ):
            base_url = os.getenv("MINIMAX_BASE_URL", "https://api.minimax.io/v1")
            if model == "gpt-4o":
                model = os.getenv("MINIMAX_MODEL", "MiniMax-M2.7")

        logger.info(f"Initializing McpManifestGeneratorAgent with model: {model}")
        self.client = OpenAI(api_key=self.api_key, base_url=base_url)
        self.model = model
        self.tools = AgentAnalyzerTools()
        self.max_iterations = 10

    def _get_system_prompt(self) -> str:
        return """You are an MCP Server Manifest Generator Agent that analyzes MCP server python code and generates McpServerManifest json.

Your goal: Analyze the server implementation to accurately discover tools and resources and produce the manifest.

Available tools:
- glob_files: Find files matching patterns (like "**/*.py")
- read_file: Read file contents
- grep_code: Search for patterns in files
- analyze_python_functions: Extract function definitions from Python files
- extract_agent_metadata: Get metadata from README, config files
- generate_mcp_manifest_json: Create the final Manifest JSON

CRITICAL WORKFLOW:

1. **Find Files**:
   - Use glob_files to locate Python files in the directory.

2. **Read Code**:
   - Look for FastMCP or mcp.server server initialization.
   - Find tools decorated with `@mcp.tool()` or explicitly added.
   - Find resources decorated with `@mcp.resource()`.

3. **Extract Metadata**:
   - Get the application name and description from the server initializer (e.g. `FastMCP("My Server")`).
   - If not explicit, derive it from the README or top-level docstring.

4. **Map Tools & Resources**:
   - For each tool, extract the `name` (function name or decorator name) and `description` (from docstring).
   - Do the same for resources.

5. **Generate Manifest**:
   - Use generate_mcp_manifest_json to produce the final payload.
   
IMPORTANT:
- Use accurate descriptions and explicitly listed names.
"""

    def _get_tool_schemas(self) -> List[Dict[str, Any]]:
        return [
            {
                "type": "function",
                "function": {
                    "name": "glob_files",
                    "description": "Find files matching a glob pattern",
                    "parameters": {"type": "object", "properties": {"pattern": {"type": "string"}, "base_path": {"type": "string"}}, "required": ["pattern"]},
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read file contents",
                    "parameters": {"type": "object", "properties": {"file_path": {"type": "string"}}, "required": ["file_path"]},
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "analyze_python_functions",
                    "description": "Extract function definitions from Python file",
                    "parameters": {"type": "object", "properties": {"file_path": {"type": "string"}}, "required": ["file_path"]},
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "extract_agent_metadata",
                    "description": "Extract metadata from agent directory",
                    "parameters": {"type": "object", "properties": {"agent_path": {"type": "string"}}, "required": ["agent_path"]},
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "generate_mcp_manifest_json",
                    "description": "Generate MCP Server Manifest JSON",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "agent_name": {"type": "string", "description": "Name of the MCP server"},
                            "description": {"type": "string", "description": "Description of the MCP server"},
                            "version": {"type": "string", "description": "Server version"},
                            "tools": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {"name": {"type": "string"}, "description": {"type": "string"}},
                                    "required": ["name", "description"]
                                }
                            },
                            "resources": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {"name": {"type": "string"}, "description": {"type": "string"}},
                                    "required": ["name", "description"]
                                }
                            }
                        },
                        "required": ["agent_name", "description", "tools", "resources"],
                    },
                },
            },
        ]

    def _execute_tool(self, tool_name: str, arguments: Dict[str, Any]) -> Dict[str, Any]:
        """Execute a tool and return result"""
        if hasattr(self.tools, tool_name):
            method = getattr(self.tools, tool_name)
            return method(**arguments)
        return {"status": "error", "message": f"Tool '{tool_name}' not found"}

    def generate_mcp_manifest(self, agent_path: str, verbose: bool = False) -> Dict[str, Any]:
        logger.info(f"Starting MCP Manifest generation for: {agent_path}")
        user_message = f"Generate an MCP Server Manifest for the server at: {agent_path}"

        messages = [
            {"role": "system", "content": self._get_system_prompt()},
            {"role": "user", "content": user_message},
        ]

        iteration = 0
        final_manifest = None
        success_tool_name = "generate_mcp_manifest_json"

        while iteration < self.max_iterations:
            iteration += 1
            if verbose:
                print(f"[Iteration {iteration}]")

            try:
                response = self.client.chat.completions.create(
                    model=self.model,
                    messages=messages,
                    tools=self._get_tool_schemas(),
                    tool_choice="auto",
                    temperature=0.1,
                    max_tokens=4000,
                )

                message = response.choices[0].message
                assistant_message = {"role": "assistant", "content": message.content or ""}
                
                if message.tool_calls:
                    assistant_message["tool_calls"] = [
                        {
                            "id": tc.id,
                            "type": "function",
                            "function": {"name": tc.function.name, "arguments": tc.function.arguments},
                        }
                        for tc in message.tool_calls
                    ]
                messages.append(assistant_message)

                if message.tool_calls:
                    for tool_call in message.tool_calls:
                        tool_name = tool_call.function.name
                        arguments = json.loads(tool_call.function.arguments)

                        result = self._execute_tool(tool_name, arguments)

                        if tool_name == success_tool_name and result.get("status") == "success":
                            final_manifest = result.get("agentcard")
                            logger.info("MCP Manifest JSON successfully generated")

                        messages.append({
                            "role": "tool",
                            "tool_call_id": tool_call.id,
                            "content": json.dumps(result),
                        })
                    continue

                break

            except Exception as e:
                logger.exception(f"Error during execution at iteration {iteration}: {e}")
                return {
                    "status": "error",
                    "message": f"Error during execution: {str(e)}",
                    "agentcard": None,
                }

        if iteration >= self.max_iterations:
            logger.warning("Maximum iterations reached")
            return {
                "status": "error",
                "message": "Maximum iterations reached",
                "agentcard": final_manifest,
            }

        return {
            "status": "success",
            "message": "MCP Manifest generated successfully",
            "agentcard": final_manifest,
            "iterations": iteration,
        }
