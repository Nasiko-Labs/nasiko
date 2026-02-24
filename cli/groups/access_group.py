"""
Access control command group.
"""

from typing import List
import typer

# Create Access Control command group
access_app = typer.Typer(help="Access control and permissions")


@access_app.command(name="grant-user")
def grant_user_access(
    agent_id: str = typer.Argument(..., help="Agent ID to grant access to"),
    user_ids: List[str] = typer.Option(..., "--user-id", "-u", help="User IDs to grant access (can specify multiple)"),
):
    """Grant access to an agent for specific users."""
    from commands.access import grant_user_access_command
    grant_user_access_command(agent_id, user_ids)


@access_app.command(name="grant-agent")
def grant_agent_access(
    agent_id: str = typer.Argument(..., help="Agent ID to grant access to"),
    target_agent_ids: List[str] = typer.Option(..., "--agent-id", "-a", help="Target agent IDs to grant access (can specify multiple)"),
):
    """Grant access to an agent for specific other agents."""
    from commands.access import grant_agent_access_command
    grant_agent_access_command(agent_id, target_agent_ids)


@access_app.command(name="list")
def list_agent_access(
    agent_id: str = typer.Argument(..., help="Agent ID to list access for"),
):
    """List current access permissions for an agent."""
    from commands.access import list_agent_access_command
    list_agent_access_command(agent_id)


@access_app.command(name="revoke-user")
def revoke_user_access(
    agent_id: str = typer.Argument(..., help="Agent ID to revoke access from"),
    user_ids: List[str] = typer.Option(..., "--user-id", "-u", help="User IDs to revoke access (can specify multiple)"),
):
    """Revoke access to an agent for specific users."""
    from commands.access import revoke_user_access_command
    revoke_user_access_command(agent_id, user_ids)


@access_app.command(name="revoke-agent")
def revoke_agent_access(
    agent_id: str = typer.Argument(..., help="Agent ID to revoke access from"),
    target_agent_ids: List[str] = typer.Option(..., "--agent-id", "-a", help="Target agent IDs to revoke access (can specify multiple)"),
):
    """Revoke access to an agent for specific other agents."""
    from commands.access import revoke_agent_access_command
    revoke_agent_access_command(agent_id, target_agent_ids)