"""
MCP Manifest Generator

Generates a lightweight manifest for MCP servers by analyzing Python source files.
"""

import ast
import json
import logging
import re
from pathlib import Path
from typing import Any, Dict, List, Optional

from .tools import AgentAnalyzerTools

logger = logging.getLogger(__name__)


class MCPManifestGeneratorAgent:
    """Generate MCP manifests from source code analysis."""

    def __init__(self):
        self.tools = AgentAnalyzerTools()

    def _slugify(self, value: str) -> str:
        value = re.sub(r"[^a-zA-Z0-9]+", "-", value.strip().lower())
        value = re.sub(r"-+", "-", value).strip("-")
        return value or "mcp-server"

    def _first_line(self, text: Optional[str]) -> str:
        if not text:
            return ""
        return text.strip().splitlines()[0].strip()

    def _decorator_name(self, decorator: ast.AST) -> str:
        target = decorator
        if isinstance(target, ast.Call):
            target = target.func

        if isinstance(target, ast.Name):
            return target.id
        if isinstance(target, ast.Attribute):
            return target.attr
        return ""

    def _decorator_source(self, decorator: ast.AST) -> str:
        try:
            return ast.unparse(decorator) if hasattr(ast, "unparse") else ""
        except Exception:
            return ""

    def _extract_string_arg(self, decorator: ast.AST) -> Optional[str]:
        if not isinstance(decorator, ast.Call):
            return None

        for arg in decorator.args:
            if isinstance(arg, ast.Constant) and isinstance(arg.value, str):
                return arg.value.strip()
        for kw in decorator.keywords:
            if isinstance(kw.value, ast.Constant) and isinstance(kw.value.value, str):
                return kw.value.value.strip()
        return None

    def _params_from_function(self, node: ast.FunctionDef) -> List[str]:
        params: List[str] = []
        for arg in node.args.args:
            if arg.arg != "self":
                params.append(arg.arg)
        return params

    def _make_examples(self, name: str, description: str, params: List[str]) -> List[str]:
        pretty_name = name.replace("_", " ").strip().title() or "Tool"
        if params:
            return [
                f"Use {pretty_name} with {', '.join(params)} to {description.rstrip('.')}",
            ]
        return [f"Use {pretty_name} to {description.rstrip('.')}"]

    def _scan_python_file(
        self, file_path: Path
    ) -> Dict[str, List[Dict[str, Any]]]:
        """Extract tool/resource/prompt candidates from a Python file."""
        try:
            content = file_path.read_text(encoding="utf-8", errors="ignore")
            tree = ast.parse(content)
        except Exception as e:
            logger.warning(f"Skipping {file_path}: {e}")
            return {"tools": [], "resources": [], "prompts": []}

        analysis = self.tools.analyze_python_functions(str(file_path))
        functions = {}
        if analysis.get("status") == "success":
            for fn in analysis.get("functions", []):
                functions[fn.get("name", "")] = fn

        tool_entries: List[Dict[str, Any]] = []
        resource_entries: List[Dict[str, Any]] = []
        prompt_entries: List[Dict[str, Any]] = []

        for node in ast.walk(tree):
            if not isinstance(node, ast.FunctionDef):
                continue
            if node.name.startswith("_"):
                continue

            decorator_sources = [self._decorator_source(d) for d in node.decorator_list]
            decorator_names = [self._decorator_name(d) for d in node.decorator_list]
            has_tool = any(
                name == "tool" or ".tool" in source.lower()
                for name, source in zip(decorator_names, decorator_sources)
            )
            has_resource = any(
                name == "resource" or ".resource" in source.lower()
                for name, source in zip(decorator_names, decorator_sources)
            )
            has_prompt = any(
                name == "prompt" or ".prompt" in source.lower()
                for name, source in zip(decorator_names, decorator_sources)
            )

            if not (has_tool or has_resource or has_prompt):
                continue

            info = functions.get(node.name, {})
            description = self._first_line(
                ast.get_docstring(node) or info.get("description")
            )
            if not description:
                description = f"{node.name.replace('_', ' ').strip().title()} handler"

            params = self._params_from_function(node)
            examples = self._make_examples(node.name, description, params)

            string_hint = None
            for decorator in node.decorator_list:
                hint = self._extract_string_arg(decorator)
                if hint:
                    string_hint = hint
                    break

            entry = {
                "name": node.name,
                "id": self._slugify(string_hint or node.name),
                "description": description,
                "examples": examples,
                "parameters": params,
                "sourceFile": str(file_path),
            }

            if has_tool:
                tool_entries.append(entry)
            if has_resource:
                resource_entries.append(
                    {
                        **entry,
                        "uri": string_hint or entry["id"],
                    }
                )
            if has_prompt:
                prompt_entries.append(
                    {
                        **entry,
                        "prompt": description,
                    }
                )

        return {
            "tools": tool_entries,
            "resources": resource_entries,
            "prompts": prompt_entries,
        }

    async def generate_and_save_mcp_manifest(
        self,
        agent_path: str,
        agent_name: str,
    ) -> bool:
        """Generate MCPManifest.json if absent and save it."""
        try:
            agent_dir = Path(agent_path)
            manifest_path = agent_dir / "MCPManifest.json"
            fallback_paths = [
                agent_dir / "mcp-manifest.json",
                agent_dir / "mcp_manifest.json",
            ]

            if manifest_path.exists() or any(path.exists() for path in fallback_paths):
                logger.info(f"MCP manifest already exists at {agent_path}")
                return False

            metadata = self.tools.extract_agent_metadata(agent_path)
            manifest_name = metadata.get("agent_name") or agent_name or agent_dir.name
            description = metadata.get("description") or (
                f"MCP server published via Nasiko: {manifest_name}"
            )

            file_result = self.tools.glob_files("**/*.py", agent_path)
            python_files = []
            if file_result.get("status") == "success":
                python_files = [Path(p) for p in file_result.get("files", [])]
            else:
                python_files = list(agent_dir.rglob("*.py"))

            tools: List[Dict[str, Any]] = []
            resources: List[Dict[str, Any]] = []
            prompts: List[Dict[str, Any]] = []

            for py_file in python_files:
                scanned = self._scan_python_file(py_file)
                tools.extend(scanned["tools"])
                resources.extend(scanned["resources"])
                prompts.extend(scanned["prompts"])

            # Preserve accuracy but avoid a totally empty manifest.
            if not tools and python_files:
                for py_file in python_files:
                    analysis = self.tools.analyze_python_functions(str(py_file))
                    if analysis.get("status") != "success":
                        continue
                    for fn in analysis.get("functions", []):
                        name = fn.get("name")
                        if not name or name.startswith("_"):
                            continue
                        description_text = fn.get("description") or (
                            f"{name.replace('_', ' ').strip().title()} handler"
                        )
                        tools.append(
                            {
                                "name": name,
                                "id": self._slugify(name),
                                "description": description_text,
                                "examples": self._make_examples(
                                    name, description_text, fn.get("parameters", [])
                                ),
                                "parameters": fn.get("parameters", []),
                                "sourceFile": str(py_file),
                            }
                        )

            manifest = {
                "schemaVersion": "0.1.0",
                "name": manifest_name,
                "description": description,
                "transport": {"type": "stdio"},
                "preferredTransport": "stdio",
                "defaultInputModes": ["text/plain"],
                "defaultOutputModes": ["text/plain"],
                "capabilities": {
                    "tools": len(tools),
                    "resources": len(resources),
                    "prompts": len(prompts),
                },
                "tools": tools,
                "resources": resources,
                "prompts": prompts,
                "sourceFiles": [str(p) for p in python_files],
            }

            with open(manifest_path, "w", encoding="utf-8") as f:
                json.dump(manifest, f, indent=2, ensure_ascii=False)

            logger.info(f"Saved MCP manifest to {manifest_path}")
            return True

        except Exception as e:
            logger.error(f"Failed to generate MCP manifest for {agent_name}: {e}")
            return False

    async def load_mcp_manifest_from_file(self, agent_path: str) -> Optional[Dict[str, Any]]:
        """Load an MCP manifest from disk if present."""
        try:
            agent_dir = Path(agent_path)
            for candidate in [
                agent_dir / "MCPManifest.json",
                agent_dir / "mcp-manifest.json",
                agent_dir / "mcp_manifest.json",
            ]:
                if candidate.exists():
                    with open(candidate, "r", encoding="utf-8") as f:
                        return json.load(f)
            return None
        except Exception as e:
            logger.error(f"Failed to load MCP manifest from {agent_path}: {e}")
            return None
