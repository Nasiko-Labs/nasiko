"""
MCP server command group.
"""

from typing import Optional
import typer

# Create MCP command group
mcp_app = typer.Typer(help="MCP server management operations")


@mcp_app.command(name="list")
def mcp_list(
    format_type: str = typer.Option(
        "table", "--format", "-f", help="Output format: table, json, list"
    ),
    show_details: bool = typer.Option(
        False, "--details", "-d", help="Show additional details"
    ),
):
    """List all published MCP servers in the registry."""
    from commands.registry import list_mcp_servers_command

    list_mcp_servers_command(format_type, show_details)


@mcp_app.command(name="get")
def mcp_get(
    agent_id: Optional[str] = typer.Option(
        None, help="MCP server agent ID"
    ),
    name: Optional[str] = typer.Option(None, "--name", help="Search by name"),
    format_type: str = typer.Option(
        "details", "--format", "-f", help="Output format: details, json"
    ),
):
    """Get detailed information about a specific MCP server."""
    search_methods = [agent_id is not None, name is not None]
    if sum(search_methods) > 1:
        typer.echo(
            "Error: Only one search method can be specified (--name or --agent-id)"
        )
        raise typer.Exit(1)
    elif sum(search_methods) == 0:
        typer.echo(
            "Error: You must specify at least one search method (--agent-id or --name)"
        )
        raise typer.Exit(1)

    if name is not None:
        identifier = name
        by_name = True
        by_agent_id = False
    else:
        identifier = agent_id
        by_name = False
        by_agent_id = True

    from commands.registry import get_agent_command

    get_agent_command(identifier, by_name, by_agent_id, format_type)


@mcp_app.command(name="register-remote")
def register_remote(
    url: str = typer.Argument(..., help="Remote MCP server URL (HTTP/SSE)"),
    name: Optional[str] = typer.Option(
        None, "--name", "-n", help="Display name for the MCP server"
    ),
):
    """Register a remote MCP server by URL (HTTP/SSE) with manifest discovery."""
    from commands.registry import register_remote_mcp_command

    register_remote_mcp_command(url, name)
