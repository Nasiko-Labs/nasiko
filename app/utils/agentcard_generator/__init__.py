"""
AgentCard Generator Agent

An agent that generates A2A-compliant AgentCards by analyzing agent code,
mimicking Claude Code's workflow.
"""

from .mcp_manifest_generator import MCPManifestGeneratorAgent
from .tools import AgentAnalyzerTools

try:
    from .agent import AgentCardGeneratorAgent
except Exception as import_error:
    class AgentCardGeneratorAgent:  # type: ignore[no-redef]
        """Fallback placeholder when optional LLM deps are unavailable."""

        def __init__(self, *args, **kwargs):
            raise ImportError(
                "AgentCardGeneratorAgent requires optional dependencies that are not installed"
            ) from import_error

__version__ = "1.0.0"
__all__ = ["AgentCardGeneratorAgent", "MCPManifestGeneratorAgent", "AgentAnalyzerTools"]
