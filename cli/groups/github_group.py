"""
GitHub command group.
"""

from typing import Optional
import typer

# Create GitHub command group
github_app = typer.Typer(help="GitHub integration and repository management")


@github_app.command(name="login")
def github_login():
    """Authenticate with GitHub via the Nasiko backend automatically."""
    from commands.github import login_command
    login_command()


@github_app.command(name="logout")
def github_logout():
    """Logout from GitHub and clear authentication session."""
    from commands.github import logout_command
    logout_command()


@github_app.command(name="repos")
def list_github_repos():
    """List your accessible GitHub repositories."""
    from commands.github import list_repos_command
    list_repos_command()


@github_app.command(name="status")
def github_status():
    """Get github status"""
    from commands.github import get_github_status
    get_github_status()


@github_app.command(name="clone")
def github_clone(
    repo: Optional[str] = typer.Argument(
        None,
        help="GitHub repository URL or name (owner/repo). If not provided, will show list to select from.",
    ),
    branch: Optional[str] = typer.Option(
        None, "--branch", "-b", help="Branch to clone (defaults to main)"
    ),
):
    """Clone a GitHub repository and upload it as an agent. If no repo specified, select from a list."""
    from commands.github import clone_command
    clone_command(repo, branch)