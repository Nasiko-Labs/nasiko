"""
Search command group.
"""

import typer

# Create Search command group
search_app = typer.Typer(help="Search users and agents")


@search_app.command(name="users")
def search_users(
    query: str = typer.Argument(..., help="Search query (minimum 2 characters)"),
    limit: int = typer.Option(10, "--limit", "-l", help="Maximum number of results (max 50)"),
):
    """Search for users with autocomplete functionality."""
    from commands.search import search_users
    search_users(query, limit)


@search_app.command(name="agents")
def search_agents(
    query: str = typer.Argument(..., help="Search query (minimum 2 characters)"),
    limit: int = typer.Option(10, "--limit", "-l", help="Maximum number of results (max 50)"),
):
    """Search for agents with autocomplete functionality."""
    from commands.search import search_agents
    search_agents(query, limit)