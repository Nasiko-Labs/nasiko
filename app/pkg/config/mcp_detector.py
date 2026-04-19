"""
MCP Server Artifact Detector
Detects whether an uploaded artifact is an MCP server or traditional AI agent.
"""

import re
from pathlib import Path
from typing import Tuple, Optional


class MCPDetectionResult:
    """Result of MCP server detection"""
    
    def __init__(self, is_mcp: bool, is_agent: bool, error: Optional[str] = None):
        self.is_mcp = is_mcp
        self.is_agent = is_agent
        self.error = error


class MCPDetector:
    """Detects MCP servers vs traditional agents"""
    
    # MCP imports and decorators to look for
    MCP_PATTERNS = [
        r'from\s+mcp\s+import',
        r'from\s+mcp\.server\s+import',
        r'from\s+mcp\.server\.fastmcp\s+import',
        r'import\s+mcp',
        r'FastMCP',
        r'@mcp\.tool',
        r'@mcp\.resource',
        r'@mcp\.prompt',
        r'mcp\.tool\(',
        r'mcp\.resource\(',
        r'mcp\.prompt\(',
        r'add_tool\(',
        r'add_resource\(',
        r'add_prompt\(',
        r'register_tool\(',
        r'register_resource\(',
        r'register_prompt\(',
    ]
    
    # Agent-specific patterns (LangChain, CrewAI, etc.)
    AGENT_PATTERNS = [
        r'from\s+langchain',
        r'import\s+langchain',
        r'from\s+crewai',
        r'import\s+crewai',
        r'LangChain',
        r'CrewAI',
        r'AgentExecutor',
    ]
    
    @staticmethod
    def find_python_entry_point(agent_path: str) -> Optional[Path]:
        """
        Find the main Python entry point (src/main.py or main.py)
        Returns the path if found, None otherwise
        """
        agent_dir = Path(agent_path)
        
        locations = [
            agent_dir / "src" / "main.py",
            agent_dir / "main.py",
            agent_dir / "src" / "__main__.py",
            agent_dir / "__main__.py",
        ]
        
        for loc in locations:
            if loc.exists() and loc.is_file():
                return loc
        
        return None
    
    @staticmethod
    def _read_python_file(file_path: Path) -> str:
        """Safely read Python file content"""
        try:
            return file_path.read_text(encoding='utf-8')
        except UnicodeDecodeError:
            # Try with default encoding
            return file_path.read_text()
        except Exception:
            return ""
    
    @staticmethod
    def _has_pattern_matches(content: str, patterns: list) -> bool:
        """Check if content matches any of the patterns"""
        for pattern in patterns:
            if re.search(pattern, content, re.IGNORECASE):
                return True
        return False
    
    @classmethod
    def detect(cls, agent_path: str) -> MCPDetectionResult:
        """
        Detect if artifact is an MCP server or traditional agent.
        
        Returns:
            MCPDetectionResult with is_mcp, is_agent, and error status
            
        Raises:
            ValueError if both MCP and agent patterns detected (ambiguous)
        """
        # Find Python entry point
        main_py_path = cls.find_python_entry_point(agent_path)
        
        if not main_py_path:
            # No Python entry point found - treat as agent (validation will catch issues)
            return MCPDetectionResult(is_mcp=False, is_agent=True)
        
        # Read Python file content
        content = cls._read_python_file(main_py_path)
        
        if not content:
            # Empty or unreadable file - treat as agent
            return MCPDetectionResult(is_mcp=False, is_agent=True)
        
        # Check for MCP patterns
        has_mcp = cls._has_pattern_matches(content, cls.MCP_PATTERNS)
        
        # Check for agent patterns
        has_agent = cls._has_pattern_matches(content, cls.AGENT_PATTERNS)
        
        # Validation logic
        if has_mcp and has_agent:
            # Both patterns detected - this is ambiguous
            return MCPDetectionResult(
                is_mcp=False,
                is_agent=False,
                error="Ambiguous artifact: contains both MCP and Agent patterns. "
                      "An artifact must be either an MCP server OR an agent, not both."
            )
        
        if has_mcp:
            return MCPDetectionResult(is_mcp=True, is_agent=False)
        
        # Default to agent if no MCP patterns found
        return MCPDetectionResult(is_mcp=False, is_agent=True)
