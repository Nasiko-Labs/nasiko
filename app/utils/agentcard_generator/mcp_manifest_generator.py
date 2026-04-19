"""
MCP Manifest Generator Agent
Uses the shared tool loop from the AgentCard generator workflow
to produce McpServerManifest.json structures.
"""

import json
import logging
import os
from pathlib import Path
from typing import Any, Dict, List

from openai import OpenAI

from .tools import AgentAnalyzerTools

logger = logging.getLogger(__name__)


class MCPManifestGeneratorAgent:
    """LLM-driven MCP manifest generator reusing shared analyzer tools."""

    def __init__(self, api_key: str = None, model: str = "gpt-4o-mini", base_url: str = None):
        self.api_key = (
            api_key or os.getenv("OPENAI_API_KEY") or os.getenv("MINIMAX_API_KEY")
        )
        if not self.api_key:
            raise ValueError("OPENAI_API_KEY or MINIMAX_API_KEY must be set")

        if (
            not base_url
            and not api_key
            and not os.getenv("OPENAI_API_KEY")
            and os.getenv("MINIMAX_API_KEY")
        ):
            base_url = os.getenv("MINIMAX_BASE_URL", "https://api.minimax.io/v1")
            if model == "gpt-4o-mini":
                model = os.getenv("MINIMAX_MODEL", "MiniMax-M2.7")

        self.client = OpenAI(api_key=self.api_key, base_url=base_url)
        self.model = model
        self.tools = AgentAnalyzerTools()
        self.max_iterations = 8

    def _get_system_prompt(self) -> str:
        return """You are an MCP Manifest Generator Agent.

Generate an MCP manifest for a server codebase. Reuse available tools and finish by calling generate_mcp_manifest_json.

Required behavior:
1. Inspect src/main.py or main.py to identify MCP declarations.
2. Detect FastMCP decorator-based declarations:
   - @mcp.tool(...)
   - @mcp.resource(...)
   - @mcp.prompt(...)
3. Detect equivalent lower-level registrations (e.g. server.tool(...), server.resource(...), server.prompt(...), add_tool/add_resource/add_prompt).
4. Manifest must include non-empty entries whenever code declares tools/resources/prompts.
5. Do not emit AgentCard schema. Only emit MCP manifest via generate_mcp_manifest_json.

Tool usage guidance:
- Use glob_files/read_file/grep_code/analyze_python_functions to inspect source.
- End with generate_mcp_manifest_json.
"""

    def _get_tool_schemas(self) -> List[Dict[str, Any]]:
        return [
            {
                "type": "function",
                "function": {
                    "name": "glob_files",
                    "description": "Find files matching a glob pattern",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "pattern": {"type": "string"},
                            "base_path": {"type": "string"},
                        },
                        "required": ["pattern"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read file contents",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "file_path": {"type": "string"},
                        },
                        "required": ["file_path"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "grep_code",
                    "description": "Search for regex pattern in a file",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "pattern": {"type": "string"},
                            "file_path": {"type": "string"},
                            "case_sensitive": {"type": "boolean"},
                        },
                        "required": ["pattern", "file_path"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "analyze_python_functions",
                    "description": "Extract Python functions from a file",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "file_path": {"type": "string"},
                        },
                        "required": ["file_path"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "extract_agent_metadata",
                    "description": "Read metadata from README/config files",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "agent_path": {"type": "string"},
                        },
                        "required": ["agent_path"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "generate_mcp_manifest_json",
                    "description": "Generate the final MCP manifest JSON",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "server_name": {"type": "string"},
                            "description": {"type": "string"},
                            "version": {"type": "string"},
                            "transport": {"type": "string"},
                            "tools": {
                                "type": "array",
                                "items": {"type": "object"},
                            },
                            "resources": {
                                "type": "array",
                                "items": {"type": "object"},
                            },
                            "prompts": {
                                "type": "array",
                                "items": {"type": "object"},
                            },
                        },
                        "required": ["server_name"],
                    },
                },
            },
        ]

    def _execute_tool(self, tool_name: str, arguments: Dict[str, Any]) -> Dict[str, Any]:
        if hasattr(self.tools, tool_name):
            return getattr(self.tools, tool_name)(**arguments)
        return {"status": "error", "message": f"Tool '{tool_name}' not found"}

    def generate_manifest(
        self, server_path: str, server_name: str = None, verbose: bool = False
    ) -> Dict[str, Any]:
        server_name = server_name or Path(server_path).name
        user_message = (
            f"Generate an MCP manifest for server '{server_name}' at path: {server_path}. "
            "Inspect code first, then call generate_mcp_manifest_json with extracted tools/resources/prompts."
        )

        messages: List[Dict[str, Any]] = [
            {"role": "system", "content": self._get_system_prompt()},
            {"role": "user", "content": user_message},
        ]

        iteration = 0
        final_manifest = None

        while iteration < self.max_iterations:
            iteration += 1

            response = self.client.chat.completions.create(
                model=self.model,
                messages=messages,
                tools=self._get_tool_schemas(),
                tool_choice="auto",
                temperature=0.1,
                max_tokens=3000,
            )

            message = response.choices[0].message
            assistant_message: Dict[str, Any] = {
                "role": "assistant",
                "content": message.content or "",
            }

            if message.tool_calls:
                assistant_message["tool_calls"] = [
                    {
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.function.name,
                            "arguments": tc.function.arguments,
                        },
                    }
                    for tc in message.tool_calls
                ]

            messages.append(assistant_message)

            if message.tool_calls:
                for tool_call in message.tool_calls:
                    tool_name = tool_call.function.name
                    arguments = json.loads(tool_call.function.arguments)
                    result = self._execute_tool(tool_name, arguments)

                    if (
                        tool_name == "generate_mcp_manifest_json"
                        and result.get("status") == "success"
                    ):
                        final_manifest = result.get("manifest")

                    messages.append(
                        {
                            "role": "tool",
                            "tool_call_id": tool_call.id,
                            "content": json.dumps(result),
                        }
                    )

                continue

            break

        if not final_manifest:
            return {
                "status": "error",
                "message": "MCP manifest generation did not produce a manifest",
                "manifest": None,
                "iterations": iteration,
            }

        if verbose:
            logger.info(f"Generated MCP manifest in {iteration} iterations")

        return {
            "status": "success",
            "message": "MCP manifest generated successfully",
            "manifest": final_manifest,
            "iterations": iteration,
        }
