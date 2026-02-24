"""
Agent access management commands for Nasiko CLI.
"""

import typer
from typing import List
from rich.console import Console

from core.api_client import get_api_client
from core.settings import APIEndpoints

console = Console()


def grant_user_access_command(agent_id: str, user_ids: List[str]):
    """
    Grant access to an agent for specific users.
    """
    console.print(f"[bold magenta]--- Granting User Access ---[/bold magenta]")
    console.print(f"[cyan]Agent ID: {agent_id}[/cyan]")
    console.print(f"[cyan]User IDs: {', '.join(user_ids)}[/cyan]")

    try:
        client = get_api_client()
        
        # Prepare request data
        request_data = {
            "user_ids": user_ids
        }
        
        console.print("[yellow]🔄 Granting user access...[/yellow]")
        
        response = client.auth_post(f"auth/agents/{agent_id}/access/users", request_data)
        result = client.handle_response(response)
        
        if result is None:
            raise typer.Exit(1)
            
        if result.get("status") == "success":
            granted_count = len(result.get("granted_users", []))
            console.print(f"[green]✅ {result.get('message', 'Access granted successfully')}[/green]")
            console.print(f"[green]Granted access to {granted_count} user(s)[/green]")
            
            # Show granted users if available
            granted_users = result.get("granted_users", [])
            if granted_users:
                console.print("[cyan]Granted users:[/cyan]")
                for user_id in granted_users:
                    console.print(f"  - {user_id}")
        else:
            console.print(f"[red]Error: {result.get('message', 'Failed to grant access')}[/red]")
            raise typer.Exit(1)

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: unexpected error while granting user access: {e}[/red]")
        raise typer.Exit(1)


def grant_agent_access_command(agent_id: str, agent_ids: List[str]):
    """
    Grant access to an agent for specific other agents.
    """
    console.print(f"[bold magenta]--- Granting Agent Access ---[/bold magenta]")
    console.print(f"[cyan]Agent ID: {agent_id}[/cyan]")
    console.print(f"[cyan]Target Agent IDs: {', '.join(agent_ids)}[/cyan]")

    try:
        client = get_api_client()
        
        # Prepare request data
        request_data = {
            "agent_ids": agent_ids
        }
        
        console.print("[yellow]🔄 Granting agent access...[/yellow]")
        
        response = client.auth_post(f"auth/agents/{agent_id}/access/agents", request_data)
        result = client.handle_response(response)
        
        if result is None:
            raise typer.Exit(1)
            
        if result.get("status") == "success":
            granted_count = len(result.get("granted_agents", []))
            console.print(f"[green]✅ {result.get('message', 'Access granted successfully')}[/green]")
            console.print(f"[green]Granted access to {granted_count} agent(s)[/green]")
            
            # Show granted agents if available
            granted_agents = result.get("granted_agents", [])
            if granted_agents:
                console.print("[cyan]Granted agents:[/cyan]")
                for agent_id_granted in granted_agents:
                    console.print(f"  - {agent_id_granted}")
        else:
            console.print(f"[red]Error: {result.get('message', 'Failed to grant access')}[/red]")
            raise typer.Exit(1)

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: unexpected error while granting agent access: {e}[/red]")
        raise typer.Exit(1)


def list_agent_access_command(agent_id: str):
    """
    List current access permissions for an agent.
    """
    console.print(f"[bold magenta]--- Agent Access Information ---[/bold magenta]")
    console.print(f"[cyan]Agent ID: {agent_id}[/cyan]")

    try:
        client = get_api_client()
        
        console.print("[yellow]🔄 Fetching access information...[/yellow]")
        
        response = client.auth_get(f"auth/agents/{agent_id}/permissions")
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)

        console.print(f"[green]✅ Access information retrieved[/green]")
        console.print(f"[cyan]Owner ID: {result.get('owner_id')}[/cyan]")
        # Display users with access
        users = result.get("can_be_accessed_by_users", [])
        if users:
            console.print(f"\n[cyan]Users with access ({len(users)}):[/cyan]")
            for user in users:
                console.print(f"  - {user}")
        else:
            console.print("\n[dim]No users have access[/dim]")

        # Display agents with access
        agents = result.get("can_be_accessed_by_agents", [])
        if agents:
            console.print(f"\n[cyan]Agents with access ({len(agents)}):[/cyan]")
            for agent in agents:
                console.print(f"  - {agent}")
        else:
            console.print("\n[dim]No agents have access[/dim]")

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: unexpected error while fetching access information: {e}[/red]")
        raise typer.Exit(1)


def revoke_user_access_command(agent_id: str, user_ids: List[str]):
    """
    Revoke access to an agent for specific users.
    """
    console.print(f"[bold magenta]--- Revoking User Access ---[/bold magenta]")
    console.print(f"[cyan]Agent ID: {agent_id}[/cyan]")
    console.print(f"[cyan]User IDs: {', '.join(user_ids)}[/cyan]")

    try:
        client = get_api_client()
        
        console.print("[yellow]🔄 Revoking user access...[/yellow]")
        
        revoked_users = []
        failed_users = []
        
        # Revoke access for each user individually
        for user_id in user_ids:
            try:
                response = client.auth_delete(f"auth/agents/{agent_id}/access/users/{user_id}")
                result = client.handle_response(response)
                
                if result and result.get("status") == "success":
                    revoked_users.append(user_id)
                    console.print(f"  [green]✓ Revoked access for user: {user_id}[/green]")
                else:
                    failed_users.append(user_id)
                    console.print(f"  [red]✗ Failed to revoke access for user: {user_id}[/red]")
            except Exception as e:
                failed_users.append(user_id)
                console.print(f"  [red]✗ Error revoking access for user {user_id}: {e}[/red]")
        
        # Summary
        if revoked_users:
            console.print(f"\n[green]✅ Successfully revoked access from {len(revoked_users)} user(s)[/green]")
            console.print("[cyan]Revoked users:[/cyan]")
            for user_id in revoked_users:
                console.print(f"  - {user_id}")
        
        if failed_users:
            console.print(f"\n[red]❌ Failed to revoke access from {len(failed_users)} user(s)[/red]")
            console.print("[red]Failed users:[/red]")
            for user_id in failed_users:
                console.print(f"  - {user_id}")
            
            if not revoked_users:  # All failed
                raise typer.Exit(1)

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: unexpected error while revoking user access: {e}[/red]")
        raise typer.Exit(1)


def revoke_agent_access_command(agent_id: str, agent_ids: List[str]):
    """
    Revoke access to an agent for specific other agents.
    """
    console.print(f"[bold magenta]--- Revoking Agent Access ---[/bold magenta]")
    console.print(f"[cyan]Agent ID: {agent_id}[/cyan]")
    console.print(f"[cyan]Target Agent IDs: {', '.join(agent_ids)}[/cyan]")

    try:
        client = get_api_client()
        
        console.print("[yellow]🔄 Revoking agent access...[/yellow]")
        
        revoked_agents = []
        failed_agents = []
        
        # Revoke access for each agent individually
        for target_agent_id in agent_ids:
            try:
                response = client.auth_delete(f"auth/agents/{agent_id}/access/agents/{target_agent_id}")
                result = client.handle_response(response)
                
                if result and result.get("status") == "success":
                    revoked_agents.append(target_agent_id)
                    console.print(f"  [green]✓ Revoked access for agent: {target_agent_id}[/green]")
                else:
                    failed_agents.append(target_agent_id)
                    console.print(f"  [red]✗ Failed to revoke access for agent: {target_agent_id}[/red]")
            except Exception as e:
                failed_agents.append(target_agent_id)
                console.print(f"  [red]✗ Error revoking access for agent {target_agent_id}: {e}[/red]")
        
        # Summary
        if revoked_agents:
            console.print(f"\n[green]✅ Successfully revoked access from {len(revoked_agents)} agent(s)[/green]")
            console.print("[cyan]Revoked agents:[/cyan]")
            for agent_id_revoked in revoked_agents:
                console.print(f"  - {agent_id_revoked}")
        
        if failed_agents:
            console.print(f"\n[red]❌ Failed to revoke access from {len(failed_agents)} agent(s)[/red]")
            console.print("[red]Failed agents:[/red]")
            for agent_id_failed in failed_agents:
                console.print(f"  - {agent_id_failed}")
            
            if not revoked_agents:  # All failed
                raise typer.Exit(1)

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: unexpected error while revoking agent access: {e}[/red]")
        raise typer.Exit(1)