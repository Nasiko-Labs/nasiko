"""
Agent command group.
"""

from typing import Optional
import typer

# Create Agent command group
agent_app = typer.Typer(help="Agent management and registry operations")


@agent_app.command(name="upload-zip")
def upload_zip(
    zip_file: str = typer.Argument(..., help="Path to the .zip file containing the agent"),
    agent_name: Optional[str] = typer.Option(
        None, "--name", "-n", help="Optional agent name (will be auto-detected if not provided)"
    ),
):
    """Upload and deploy an agent from a .zip file."""
    from commands.upload_agent import upload_zip_command
    upload_zip_command(zip_file, agent_name)


@agent_app.command(name="upload-directory")
def upload_directory(
    directory_path: str = typer.Argument(..., help="Path to the directory containing the agent"),
    agent_name: Optional[str] = typer.Option(
        None, "--name", "-n", help="Optional agent name (will be auto-detected if not provided)"
    ),
):
    """Upload and deploy an agent from a local directory."""
    from commands.upload_agent import upload_directory_command
    upload_directory_command(directory_path, agent_name)


@agent_app.command(name="list-uploaded")
def list_uploaded_agents():
    """List user uploaded agents."""
    from commands.upload_agent import list_user_uploaded_agents_command
    list_user_uploaded_agents_command()


@agent_app.command(name="list")
def registry_list(
    format_type: str = typer.Option(
        "table", "--format", "-f", help="Output format: table, json, list"
    ),
    show_details: bool = typer.Option(False, "--details", "-d", help="Show additional details"),
):
    """List all agents in the registry."""
    from commands.registry import list_agents_command
    list_agents_command(format_type, show_details)


@agent_app.command(name="get")
def registry_get(
    agent_id: Optional[str] = typer.Option(
        None, help="Agent ID - searches by agent id from the registry"
    ),
    name: Optional[str] = typer.Option(
        None, "--name", help="Search by agent name"
    ),
    format_type: str = typer.Option(
        "details", "--format", "-f", help="Output format: details, json"
    ),
):
    """Get detailed information about a specific agent by agent ID, or name."""

    # Validate that only one search method is specified
    search_methods = [agent_id is not None, name is not None]
    if sum(search_methods) > 1:
        typer.echo("Error: Only one search method can be specified (--name or --agent-id)")
        raise typer.Exit(1)
    elif sum(search_methods) == 0:
        typer.echo(
            "Error: You must specify at least one search method (--agent-id or --name)"
        )
        raise typer.Exit(1)

    # Determine which identifier to use and which search method
    if name is not None:
        identifier = name
        by_name = True
        by_agent_id = False
    else:
        # Default: search by agent_id
        identifier = agent_id
        by_name = False
        by_agent_id = True

    from commands.registry import get_agent_command
    get_agent_command(identifier, by_name, by_agent_id, format_type)
