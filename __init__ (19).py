"""
Calculator MCP Server -- Sample for Nasiko Platform

A stdio-based MCP server built on the official Python MCP SDK.
Demonstrates @mcp.tool(), @mcp.resource(), and @mcp.prompt() decorators.

To publish to Nasiko:
    zip -r calculator.zip mcp-calculator-server/
    # Upload via POST /ingest or the Nasiko web app

The platform will:
    1. Auto-detect this as an MCP server (from mcp/fastmcp imports)
    2. Generate McpServerManifest.json with all tools, resources, prompts
    3. Deploy with a stdio-to-HTTP bridge
    4. Register with Kong for agent discoverability
    5. Inject OpenTelemetry tracing on every tool call
"""
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("calculator")


# -- Tools ------------------------------------------------------------------

@mcp.tool()
def add(a: int, b: int) -> int:
    """Add two numbers together."""
    return a + b


@mcp.tool()
def multiply(x: float, y: float) -> float:
    """Multiply two numbers."""
    return x * y


@mcp.tool(name="divide")
def safe_divide(numerator: float, denominator: float) -> str:
    """Safely divide two numbers. Returns error message for division by zero."""
    if denominator == 0:
        return "Error: Division by zero"
    return str(numerator / denominator)


@mcp.tool()
def power(base: float, exponent: int) -> float:
    """Raise base to the power of exponent."""
    return base ** exponent


# -- Resources --------------------------------------------------------------

@mcp.resource("config://calculator/settings")
def get_settings() -> str:
    """Return calculator configuration."""
    return '{"precision": 10, "mode": "scientific", "max_history": 100}'


@mcp.resource("data://calculator/history")
def get_history() -> str:
    """Return recent calculation history."""
    return '[]'


# -- Prompts ----------------------------------------------------------------

@mcp.prompt()
def math_helper(problem: str, show_steps: bool = True) -> str:
    """Generate a prompt for solving a math problem step by step."""
    steps = "Show your work step by step." if show_steps else ""
    return f"Solve this math problem: {problem}. {steps}"


@mcp.prompt(name="code_review")
def review_code(code: str, language: str = "python") -> str:
    """Generate a code review prompt for the given code."""
    return f"Review this {language} code for correctness and style:\n\n{code}"


if __name__ == "__main__":
    mcp.run()
