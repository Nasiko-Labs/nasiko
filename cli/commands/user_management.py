"""
User management commands for Nasiko CLI.
"""

import typer
from typing import Optional, List
from rich.console import Console

from core.api_client import get_api_client
from core.settings import APIEndpoints

console = Console()


def register_user_command(username: str, email: str, is_super_user: bool = False):
    """
    Register a new user in the system.
    """
    console.print("[bold magenta]--- User Registration ---[/bold magenta]")
    console.print(f"[cyan]Username: {username}[/cyan]")
    console.print(f"[cyan]Email: {email}[/cyan]")
    console.print(f"[cyan]Super User: {is_super_user}[/cyan]")

    try:
        client = get_api_client()
        
        # Prepare request data
        request_data = {
            "username": username,
            "email": email,
            "is_super_user": is_super_user
        }
        
        console.print("[yellow]🔄 Registering user...[/yellow]")
        
        response = client.auth_post("auth/users/register", request_data)
        result = client.handle_response(response)
        
        if result is None:
            raise typer.Exit(1)
            
        console.print(f"[green]✅ User registered successfully![/green]")
        console.print(f"[green]User ID: {result.get('user_id')}[/green]")
        # console.print(f"[green]Username: {result.get('username')}[/green]")
        # console.print(f"[green]Email: {result.get('email')}[/green]")
        console.print(f"[green]Role: {result.get('role', 'User')}[/green]")
        console.print(f"[green]Status: {result.get('status', 'Active')}[/green]")
        
        # Display credentials
        console.print("\n[bold yellow]🔑 Access Credentials (SAVE THESE SECURELY):[/bold yellow]")
        console.print(f"[yellow]Access Key: {result.get('access_key')}[/yellow]")
        console.print(f"[yellow]Access Secret: {result.get('access_secret')}[/yellow]")
        console.print("[red]⚠️ Warning: Access secret won't be shown again![/red]")

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: unexpected error during user registration: {e}[/red]")
        raise typer.Exit(1)


def list_users_command(limit: int = 50):
    """
    List all users in the system.
    """
    console.print("[bold magenta]--- All Users ---[/bold magenta]")

    try:
        client = get_api_client()
        
        console.print("[yellow]🔄 Fetching users...[/yellow]")
        
        response = client.auth_get(f"auth/users?limit={limit}")
        result = client.handle_response(response)
        
        if result is None:
            raise typer.Exit(1)
            
        if not result or len(result) == 0:
            console.print("[yellow]No users found.[/yellow]")
            return
            
        console.print(f"\n[bold magenta]Found {len(result)} users:[/bold magenta]")
        
        for i, user in enumerate(result, 1):
            user_id = user.get("user_id", "N/A")
            username = user.get("username", "N/A")
            email = user.get("email", "N/A")
            is_super = user.get("is_super_user", False)
            is_active = user.get("is_active", True)
            created_at = user.get("created_at", "N/A")
            last_login = user.get("last_login", "Never")
            
            role = "Super User" if is_super else "User"
            status = "Active" if is_active else "Inactive"
            
            console.print(f"\n  [cyan]{i:2}.[/cyan] [green]{username}[/green] ({email})")
            console.print(f"      [dim]ID: {user_id}[/dim]")
            console.print(f"      [yellow]{role}[/yellow] • [blue]{status}[/blue]")
            console.print(f"      [dim]Created: {created_at}[/dim]")
            console.print(f"      [dim]Last Login: {last_login}[/dim]")

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: unexpected error fetching users: {e}[/red]")
        raise typer.Exit(1)


def get_user_command(user_id: str):
    """
    Get detailed information about a specific user.
    """
    console.print(f"[bold magenta]--- User Details ---[/bold magenta]")
    console.print(f"[cyan]User ID: {user_id}[/cyan]")

    try:
        client = get_api_client()
        
        console.print("[yellow]🔄 Fetching user details...[/yellow]")
        
        response = client.auth_get(f"auth/users/{user_id}")
        result = client.handle_response(response)
        
        if result is None:
            raise typer.Exit(1)
            
        console.print(f"[green]✅ User found[/green]")
        
        username = result.get("username", "N/A")
        email = result.get("email", "N/A")
        is_super = result.get("is_super_user", False)
        is_active = result.get("is_active", True)
        created_at = result.get("created_at", "N/A")
        last_login = result.get("last_login", "Never")
        created_by = result.get("created_by", "System")
        
        role = "Super User" if is_super else "User"
        status = "Active" if is_active else "Inactive"
        
        console.print(f"\n[bold green]{username}[/bold green] ({email})")
        console.print(f"[yellow]Role: {role}[/yellow]")
        console.print(f"[blue]Status: {status}[/blue]")
        console.print(f"[dim]Created: {created_at}[/dim]")
        console.print(f"[dim]Last Login: {last_login}[/dim]")
        console.print(f"[dim]Created By: {created_by}[/dim]")

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: unexpected error fetching user details: {e}[/red]")
        raise typer.Exit(1)


def regenerate_credentials_command(user_id: str):
    """
    Regenerate access credentials for a user.
    """
    console.print(f"[bold magenta]--- Regenerate Credentials ---[/bold magenta]")
    console.print(f"[cyan]User ID: {user_id}[/cyan]")

    try:
        client = get_api_client()
        
        console.print("[yellow]🔄 Regenerating credentials...[/yellow]")
        
        response = client.auth_post(f"auth/users/{user_id}/regenerate-credentials", data=None)
        result = client.handle_response(response)
        
        if result is None:
            raise typer.Exit(1)
            
        console.print(f"[green]✅ {result.get('message', 'Credentials regenerated successfully')}[/green]")
        
        # Display new credentials
        console.print("\n[bold yellow]🔑 New Access Credentials (SAVE THESE SECURELY):[/bold yellow]")
        console.print(f"[yellow]Access Key: {result.get('access_key')}[/yellow]")
        console.print(f"[yellow]Access Secret: {result.get('access_secret')}[/yellow]")
        console.print("[red]⚠️ Warning: Access secret won't be shown again![/red]")

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: unexpected error regenerating credentials: {e}[/red]")
        raise typer.Exit(1)


def revoke_user_command(user_id: str):
    """
    Revoke all tokens for a specific user.
    """
    console.print(f"[bold magenta]--- Revoke User Tokens ---[/bold magenta]")
    console.print(f"[cyan]User ID: {user_id}[/cyan]")

    try:
        client = get_api_client()
        
        console.print("[yellow]🔄 Revoking all tokens for user...[/yellow]")
        
        response = client.auth_post(f"auth/tokens/revoke-user/{user_id}", data=None)
        result = client.handle_response(response)
        
        if result is None:
            raise typer.Exit(1)
            
        if result.get("status") == "success":
            revoked_count = result.get("revoked_count", 0)
            console.print(f"[green]✅ {result.get('message', 'Tokens revoked successfully')}[/green]")
            console.print(f"[green]Revoked {revoked_count} token(s)[/green]")
        else:
            console.print(f"[red]Error: {result.get('message', 'Failed to revoke tokens')}[/red]")
            raise typer.Exit(1)

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: unexpected error revoking user tokens: {e}[/red]")
        raise typer.Exit(1)


def reinstate_user_command(user_id: str):
    """
    Reinstate a user and regenerate credentials.
    """
    console.print(f"[bold magenta]--- Reinstate User ---[/bold magenta]")
    console.print(f"[cyan]User ID: {user_id}[/cyan]")

    try:
        client = get_api_client()
        
        console.print("[yellow]🔄 Reinstating user and generating new credentials...[/yellow]")
        
        response = client.auth_post(f"auth/users/{user_id}/reinstate", data=None)
        result = client.handle_response(response)
        
        if result is None:
            raise typer.Exit(1)
            
        console.print(f"[green]✅ {result.get('message', 'User reinstated successfully')}[/green]")
        console.print(f"[green]User ID: {result.get('user_id')}[/green]")
        console.print(f"[green]Username: {result.get('username')}[/green]")
        console.print(f"[green]Email: {result.get('email')}[/green]")
        console.print(f"[green]Role: {result.get('role', 'User')}[/green]")
        console.print(f"[green]Status: {result.get('status', 'Active')}[/green]")
        console.print(f"[green]Created On: {result.get('created_on')}[/green]")
        
        # Display new credentials
        console.print("\n[bold yellow]🔑 New Access Credentials (SAVE THESE SECURELY):[/bold yellow]")
        console.print(f"[yellow]Access Key: {result.get('access_key')}[/yellow]")
        console.print(f"[yellow]Access Secret: {result.get('access_secret')}[/yellow]")
        console.print("[red]⚠️ Warning: Access secret won't be shown again![/red]")

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: unexpected error reinstating user: {e}[/red]")
        raise typer.Exit(1)


def delete_user_command(user_id: str, confirm: bool = False):
    """
    Delete a user permanently.
    """
    console.print(f"[bold magenta]--- Delete User ---[/bold magenta]")
    console.print(f"[cyan]User ID: {user_id}[/cyan]")

    # Confirmation check
    if not confirm:
        console.print("[red]⚠️ WARNING: This action will permanently delete the user![/red]")
        confirmation = typer.confirm("Are you sure you want to delete this user?")
        if not confirmation:
            console.print("[yellow]Operation cancelled.[/yellow]")
            raise typer.Exit(0)

    try:
        client = get_api_client()
        
        console.print("[yellow]🔄 Deleting user permanently...[/yellow]")
        
        response = client.auth_delete(f"auth/users/{user_id}")
        result = client.handle_response(response)
        
        if result is None:
            raise typer.Exit(1)
            
        if result.get("status") == "success":
            console.print(f"[green]✅ {result.get('message', 'User deleted successfully')}[/green]")
            console.print(f"[green]Deleted User ID: {result.get('deleted_user_id')}[/green]")
        else:
            console.print(f"[red]Error: {result.get('message', 'Failed to delete user')}[/red]")
            raise typer.Exit(1)

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: unexpected error deleting user: {e}[/red]")
        raise typer.Exit(1)