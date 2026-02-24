"""
N8N command group.
"""

from typing import Optional
import typer

# Create N8N command group
n8n_app = typer.Typer(help="N8N workflow integration")


@n8n_app.command(name="register")
def n8n_register(
    workflow_id: str = typer.Argument(..., help="N8N workflow ID"),
    agent_name: Optional[str] = typer.Option(None, "--name", "-n", help="Custom agent name (auto-generated if not provided)"),
    agent_description: Optional[str] = typer.Option(
        None, "--description", "-d", help="Agent description"
    ),
):
    """Register an N8N workflow as an agent."""
    from commands.n8n import register_workflow
    register_workflow(workflow_id, agent_name, agent_description)


@n8n_app.command(name="connect")
def n8n_connect(
    n8n_url: str = typer.Option(..., "--url", prompt="Instance URL", help="N8N instance URL"),
    api_key: str = typer.Option(..., "--api-key", prompt="API Key", hide_input=True, help="N8N API key"),
    connection_name: str = typer.Option(..., "--connection-name", prompt="Connection Name", help="Name for this connection"),
):
    """Save and test N8N credentials."""
    from commands.n8n import connect_n8n
    connect_n8n(connection_name, n8n_url, api_key)


@n8n_app.command(name="credentials")
def n8n_credentials():
    """Get saved N8N credentials."""
    from commands.n8n import get_n8n_credentials
    get_n8n_credentials()


@n8n_app.command(name="update")
def n8n_update(
    connection_name: Optional[str] = typer.Option(None, "--name", help="New connection name"),
    n8n_url: Optional[str] = typer.Option(None, "--url", help="New N8N instance URL"),
    api_key: Optional[str] = typer.Option(None, "--api-key", help="New API key"),
    is_active: Optional[bool] = typer.Option(None, "--active", help="Enable/disable credential"),
):
    """Update N8N credentials."""
    from commands.n8n import update_n8n_credentials
    update_n8n_credentials(connection_name, n8n_url, api_key, is_active)


@n8n_app.command(name="delete")
def n8n_delete():
    """Delete N8N credentials permanently."""
    from commands.n8n import delete_n8n_credentials
    delete_n8n_credentials()


@n8n_app.command(name="workflows")
def n8n_workflows(
    active_only: bool = typer.Option(
        True, "--active-only", help="Filter to show only active workflows"
    ),
    limit: int = typer.Option(100, "--limit", "-l", help="Maximum number of workflows to return"),
):
    """List N8N workflows from connected instance."""
    from commands.n8n import list_n8n_workflows
    list_n8n_workflows(active_only, limit)