"""MCP manifest generator.

Reads Python source files, extracts tool, resource, and prompt definitions
via the frozen parser module, and writes atomic JSON manifests to
/tmp/nasiko/{artifact_id}/manifest.json.

Part of the Nasiko MCP Manifest Generator (R3).
"""

from __future__ import annotations

import json
import os
import re
import tempfile
from datetime import datetime, timezone
from typing import TypedDict

from .parser import (
    ToolDefinition,
    ResourceDefinition,
    PromptDefinition,
    parse_tools,
    parse_all,
)

# ---------------------------------------------------------------------------
class InputSchema(TypedDict):
    type: str
    properties: dict[str, dict]
    required: list[str]


class MCPTool(TypedDict):
    name: str
    description: str | None
    input_schema: InputSchema


class MCPResource(TypedDict):
    uri: str
    name: str
    description: str | None


class MCPPrompt(TypedDict):
    name: str
    description: str | None
    input_schema: InputSchema


class MCPManifest(TypedDict):
    artifact_id: str
    generated_at: str
    tools: list[MCPTool]
    resources: list[MCPResource]
    prompts: list[MCPPrompt]

# ---------------------------------------------------------------------------
_ARTIFACT_ID_RE = re.compile(r"^[a-zA-Z0-9_\-]+$")
_MANIFEST_ROOT = "/tmp/nasiko"
ALLOWED_SOURCE_ROOT: str = os.environ.get(
    "NASIKO_SOURCE_ROOT", "/tmp/nasiko/uploads"
)

# ---------------------------------------------------------------------------
def _validate_artifact_id(artifact_id: str) -> None:
    """Raise ``ValueError`` if *artifact_id* is empty or contains unsafe chars."""

    if not artifact_id:
        raise ValueError("artifact_id must not be empty")

    if not _ARTIFACT_ID_RE.match(artifact_id):
        raise ValueError("Invalid artifact_id")


def _build_input_schema(tool: ToolDefinition) -> InputSchema:
    """Convert a ``ToolDefinition``'s parameters into a JSON-Schema object."""

    properties: dict[str, dict] = {
        p["name"]: p["json_schema"]
        for p in tool["parameters"]
    }
    required: list[str] = [
        p["name"]
        for p in tool["parameters"]
        if p["required"]
    ]

    return InputSchema(
        type="object",
        properties=properties,
        required=required,
    )


def _build_prompt_schema(prompt: PromptDefinition) -> InputSchema:
    """Convert a ``PromptDefinition``'s parameters into a JSON-Schema object."""

    properties: dict[str, dict] = {
        p["name"]: p["json_schema"]
        for p in prompt["parameters"]
    }
    required: list[str] = [
        p["name"]
        for p in prompt["parameters"]
        if p["required"]
    ]

    return InputSchema(
        type="object",
        properties=properties,
        required=required,
    )


def _validate_source_path(source_path: str) -> None:
    """Raise ValueError if source_path escapes ALLOWED_SOURCE_ROOT.

    Uses os.path.realpath() to resolve symlinks before comparison,
    preventing symlink traversal attacks.
    """
    real = os.path.realpath(os.path.abspath(source_path))
    allowed_root = os.environ.get("NASIKO_SOURCE_ROOT", ALLOWED_SOURCE_ROOT)
    allowed = os.path.realpath(os.path.abspath(allowed_root))
    if not (real.startswith(allowed + os.sep) or real == allowed):
        raise ValueError(
            f"source_path outside allowed root: {source_path!r}"
        )


def _tool_to_mcp(tool: ToolDefinition) -> MCPTool:
    """Convert a ``ToolDefinition`` from the parser into an ``MCPTool``."""

    # description is None when the tool has no docstring.
    # Downstream consumers (R4) must handle None explicitly.
    return MCPTool(
        name=tool["name"],
        description=tool["docstring"],
        input_schema=_build_input_schema(tool),
    )


def _resource_to_mcp(resource: ResourceDefinition) -> MCPResource:
    """Convert a ``ResourceDefinition`` into an ``MCPResource``."""
    return MCPResource(
        uri=resource["uri"],
        name=resource["name"],
        description=resource["docstring"],
    )


def _prompt_to_mcp(prompt: PromptDefinition) -> MCPPrompt:
    """Convert a ``PromptDefinition`` into an ``MCPPrompt``."""
    return MCPPrompt(
        name=prompt["name"],
        description=prompt["docstring"],
        input_schema=_build_prompt_schema(prompt),
    )

def generate_manifest(artifact_id: str, source_path: str) -> MCPManifest:
    """Parse *source_path* for MCP definitions and write a manifest to disk.

    The manifest is written atomically to
    ``/tmp/nasiko/{artifact_id}/manifest.json`` and the parsed
    ``MCPManifest`` dict is returned directly.

    Raises
    ------
    ValueError
        If *artifact_id* is empty or contains path-traversal characters,
        or if *source_path* contains a Python syntax error (propagated
        from ``parse_all``).
    FileNotFoundError
        If *source_path* does not exist (propagated from ``open``).
    """

    _validate_artifact_id(artifact_id)
    _validate_source_path(source_path)

    # 1. Read source
    with open(source_path, "r", encoding="utf-8") as f:
        source_code = f.read()

    # 2. Parse all definitions (may raise ValueError on syntax errors)
    tool_defs, resource_defs, prompt_defs = parse_all(source_code)

    # 3. Build manifest
    manifest: MCPManifest = MCPManifest(
        artifact_id=artifact_id,
        generated_at=datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        tools=[_tool_to_mcp(t) for t in tool_defs],
        resources=[_resource_to_mcp(r) for r in resource_defs],
        prompts=[_prompt_to_mcp(p) for p in prompt_defs],
    )

    # 4. Write atomically
    manifest_dir = os.path.join(_MANIFEST_ROOT, artifact_id)
    os.makedirs(manifest_dir, exist_ok=True)
    manifest_path = os.path.join(manifest_dir, "manifest.json")

    fd, tmp_path = tempfile.mkstemp(dir=manifest_dir, suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(manifest, f, indent=2)
        os.replace(tmp_path, manifest_path)
    except Exception:
        os.unlink(tmp_path)
        raise

    # 5. Return the dict -- not a file path
    return manifest


def load_manifest(artifact_id: str) -> MCPManifest:
    """Load a previously generated manifest from disk.

    Raises
    ------
    ValueError
        If *artifact_id* is empty or contains path-traversal characters.
    FileNotFoundError
        If no manifest exists for the given *artifact_id*.
    """

    _validate_artifact_id(artifact_id)

    manifest_path = os.path.join(_MANIFEST_ROOT, artifact_id, "manifest.json")

    try:
        with open(manifest_path, "r", encoding="utf-8") as f:
            return json.load(f)
    except FileNotFoundError:
        raise FileNotFoundError(
            f"Manifest not found for artifact_id={artifact_id!r}"
        )
