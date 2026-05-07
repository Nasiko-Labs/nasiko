"""MCP Manifest Generator package.

Sibling to agentcard_generator/ — this generator handles MCP server artifacts.
Exports the core parts:
- router: FastAPI APIRouter for integration with Nasiko.
- generate_manifest: Core logic for manifest generation.
- load_manifest: Logic for manifest retrieval.
- parse_tools: Parser for MCP tool/resource/prompt decorators.
"""

from .endpoints import router
from .generator import generate_manifest, load_manifest
from .parser import parse_tools

__all__ = ["router", "generate_manifest", "load_manifest", "parse_tools"]
