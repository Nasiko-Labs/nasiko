"""
AgentCard Generator Agent

An agent that generates A2A-compliant AgentCards by analyzing agent code,
mimicking Claude Code's workflow.
"""

from .agent import AgentCardGeneratorAgent
from .tools import AgentAnalyzerTools

__version__ = "1.0.0"
__all__ = ["AgentCardGeneratorAgent", "AgentAnalyzerTools"]