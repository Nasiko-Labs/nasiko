"""
Authentication commands for Nasiko CLI.
"""

import typer
from typing import Optional
import sys
import os

sys.path.append(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from auth.auth_manager import get_auth_manager
from core.api_client import get_api_client

# Create auth command group
auth_app = typer.Typer(help="Authentication commands")


@auth_app.command("login")
def login_command(
    access_key: Optional[str] = typer.Option(
        None, "--access-key", "-k", help="Your access key"
    ),
    access_secret: Optional[str] = typer.Option(
        None, "--access-secret", "-s", help="Your access secret"
    ),
    save_credentials: bool = typer.Option(
        True, "--save-credentials/--no-save", help="Save credentials for auto-renewal"
    ),
    api_url: Optional[str] = typer.Option(
        None, "--api-url", help="API base URL (optional)"
    ),
):
    """Login to Nasiko with access key and secret."""

    # Get credentials interactively if not provided
    if not access_key:
        access_key = typer.prompt("Access Key")

    if not access_secret:
        access_secret = typer.prompt("Access Secret", hide_input=True)

    # Validate inputs
    if not access_key or not access_secret:
        typer.echo("❌ Access key and secret are required")
        raise typer.Exit(1)

    if not access_key.startswith("NASK_"):
        typer.echo("❌ Invalid access key format (should start with NASK_)")
        raise typer.Exit(1)

    # Login with auth manager
    auth_manager = get_auth_manager()
    if api_url:
        auth_manager.base_url = api_url

    typer.echo("🔐 Authenticating...")

    if auth_manager.login(access_key, access_secret, save_credentials):
        # Get user info if available
        user_info = auth_manager.get_user_info()
        if user_info:
            username = user_info.get("username", "Unknown")
            is_super = user_info.get("is_super_user", False)
            role = "Super User" if is_super else "User"
            typer.echo(f"👋 Welcome back, {username} ({role})")

        typer.echo("\n🚀 You can now use authenticated commands:")
        typer.echo("   • nasiko agent deploy .")
        typer.echo("   • nasiko agent list")
        typer.echo("   • nasiko auth status")
        typer.echo("   • nasiko chat start <agent>")
    else:
        raise typer.Exit(1)


@auth_app.command("logout")
def logout_command(
    clear_all: bool = typer.Option(
        False, "--clear-all", help="Clear all stored credentials"
    )
):
    """Logout from Nasiko."""

    auth_manager = get_auth_manager()

    if not auth_manager.is_logged_in():
        typer.echo("ℹ️  You are not logged in")
        return

    if auth_manager.logout(clear_credentials=clear_all):
        if clear_all:
            typer.echo("🗑️  All authentication data cleared")
        typer.echo("👋 See you next time!")
    else:
        typer.echo("⚠️  Logout may not have completed successfully")


@auth_app.command("status")
def status_command():
    """Check authentication status and show current user information."""

    auth_manager = get_auth_manager()

    if not auth_manager.is_logged_in():
        typer.echo("❌ Not logged in")
        typer.echo("\n💡 To login:")
        typer.echo("   nasiko auth login")
        return

    typer.echo("✅ Logged in")

    user_info = auth_manager.get_user_info()
    if user_info:
        typer.echo(f"   Username: {user_info.get('username', 'Unknown')}")
        typer.echo(f"   Email: {user_info.get('email', 'Not available')}")
        typer.echo(
            f"   Role: {'Super User' if user_info.get('is_super_user') else 'User'}"
        )
        typer.echo(f"   Active: {'Yes' if user_info.get('is_active') else 'No'}")

        if user_info.get("created_at"):
            typer.echo(f"   Created: {user_info['created_at']}")

        if user_info.get("last_login"):
            typer.echo(f"   Last login: {user_info['last_login']}")

    # Test API connectivity
    try:
        client = get_api_client()
        response = client.get("healthcheck", require_auth=False)
        if response.status_code == 200:
            typer.echo("   API: ✅ Connected")
        else:
            typer.echo("   API: ⚠️  Connection issues")
    except Exception:
        typer.echo("   API: ❌ Cannot connect")


if __name__ == "__main__":
    auth_app()
