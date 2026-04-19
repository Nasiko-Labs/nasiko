"""
MCP command group.
"""

from typing import List
import typer

# Create MCP command group
mcp_app = typer.Typer(help="MCP server publishing and association commands")


@mcp_app.command(name="list")
def list_mcp_servers(
    format_type: str = typer.Option(
        "table", "--format", "-f", help="Output format: table, json"
    ),
):
    """List published MCP servers."""
    from commands.mcp import list_mcp_servers_command

    list_mcp_servers_command(format_type)


@mcp_app.command(name="manifest")
def get_manifest(
    server_id: str = typer.Argument(..., help="MCP server ID"),
    format_type: str = typer.Option(
        "pretty", "--format", "-f", help="Output format: pretty, json"
    ),
):
    """Get generated MCP manifest for one server."""
    from commands.mcp import get_mcp_manifest_command

    get_mcp_manifest_command(server_id, format_type)


@mcp_app.command(name="associations")
def get_associations(
    agent_id: str = typer.Argument(..., help="Agent ID"),
):
    """Show MCP associations configured for an agent."""
    from commands.mcp import get_mcp_associations_command

    get_mcp_associations_command(agent_id)


@mcp_app.command(name="associate")
def associate_agent(
    agent_id: str = typer.Argument(..., help="Agent ID"),
    server_ids: List[str] = typer.Argument(
        ..., help="One or more MCP server IDs to associate"
    ),
    replace: bool = typer.Option(
        False,
        "--replace",
        help="Replace existing associations instead of merging",
    ),
):
    """Associate an agent with one or more MCP servers."""
    from commands.mcp import set_mcp_associations_command

    set_mcp_associations_command(agent_id, server_ids, replace)
