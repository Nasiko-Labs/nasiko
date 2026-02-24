"""
User management command group.
"""

import typer

# Create User Management command group
user_app = typer.Typer(help="User management (Super User only)")


@user_app.command(name="register")
def user_register(
    username: str = typer.Option(..., "--username", "-u", prompt="Username", help="Username for the new user"),
    email: str = typer.Option(..., "--email", "-e", prompt="Email", help="Email address for the new user"),
    is_super_user: bool = typer.Option(False, "--super-user", "-s", help="Create as super user"),
):
    """Register a new user in the system (Super User Only)."""
    from commands.user_management import register_user_command
    register_user_command(username, email, is_super_user)


@user_app.command(name="list")
def user_list(
    limit: int = typer.Option(50, "--limit", "-l", help="Maximum number of users to show"),
):
    """List all users in the system (Super User Only)."""
    from commands.user_management import list_users_command
    list_users_command(limit)


@user_app.command(name="get")
def user_get(
    user_id: str = typer.Argument(..., help="User ID to get details for"),
):
    """Get detailed information about a specific user (Super User Only)."""
    from commands.user_management import get_user_command
    get_user_command(user_id)


@user_app.command(name="regenerate-credentials")
def user_regenerate_credentials(
    user_id: str = typer.Argument(..., help="User ID to regenerate credentials for"),
):
    """Regenerate access credentials for a user (Super User Only)."""
    from commands.user_management import regenerate_credentials_command
    regenerate_credentials_command(user_id)


@user_app.command(name="revoke")
def user_revoke(
    user_id: str = typer.Argument(..., help="User ID to revoke all tokens for"),
):
    """Revoke all tokens for a specific user (Super User Only)."""
    from commands.user_management import revoke_user_command
    revoke_user_command(user_id)


@user_app.command(name="reinstate")
def user_reinstate(
    user_id: str = typer.Argument(..., help="User ID to reinstate"),
):
    """Reinstate a user and regenerate credentials (Super User Only)."""
    from commands.user_management import reinstate_user_command
    reinstate_user_command(user_id)


@user_app.command(name="delete")
def user_delete(
    user_id: str = typer.Argument(..., help="User ID to delete permanently"),
    confirm: bool = typer.Option(False, "--confirm", help="Skip confirmation prompt"),
):
    """Delete a user permanently (Super User Only)."""
    from commands.user_management import delete_user_command
    delete_user_command(user_id, confirm)