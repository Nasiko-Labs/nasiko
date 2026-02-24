"""
Search commands for Nasiko CLI.
"""

import typer
from rich.console import Console
from rich.table import Table

from core.settings import APIEndpoints
from core.api_client import get_api_client

console = Console()


def search_users(query: str, limit: int = 10):
    """Search for users with autocomplete functionality."""

    if len(query) < 2:
        console.print("[red]Error: Query must be at least 2 characters[/red]")
        raise typer.Exit(1)

    try:
        client = get_api_client()
        params = {
            "q": query,
            "limit": min(limit, 50)
        }

        response = client.get(APIEndpoints.SEARCH_USERS, params=params)
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)
            
        # Response format: {data: List[UserSearchResult], query, total_matches, showing, status_code, message}
        users = result.get('data', [])
        total_matches = result.get('total_matches', 0)
        showing = result.get('showing', len(users))

        if users:
            console.print(f"[bold magenta]User Search Results (showing {showing} of {total_matches} matches)[/bold magenta]\n")

            table = Table(show_header=True, header_style="bold magenta")
            table.add_column("Username", style="cyan", width=20)
            table.add_column("User ID", style="blue", width=40)
            table.add_column("Display Name", style="green", width=20)
            table.add_column("Email", style="yellow", width=30)
            table.add_column("Role", style="magenta", width=15)

            for user in users:
                table.add_row(
                    user.get('username', 'N/A'),
                    user.get('id', 'N/A'),
                    user.get('display_name', 'N/A'),
                    user.get('email', 'N/A'),
                    user.get('role', 'N/A')
                )

            console.print(table)
        else:
            console.print(f"[yellow]No users found matching '{query}'[/yellow]")

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")
        raise typer.Exit(1)


def search_agents(query: str, limit: int = 10):
    """Search for agents with autocomplete functionality."""

    if len(query) < 2:
        console.print("[red]Error: Query must be at least 2 characters[/red]")
        raise typer.Exit(1)

    try:
        client = get_api_client()
        params = {
            "q": query,
            "limit": min(limit, 50)
        }

        response = client.get(APIEndpoints.SEARCH_AGENTS, params=params)
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)
            
        # Response format: {data: List[AgentSearchResult], query, total_matches, showing, status_code, message}
        agents = result.get('data', [])
        total_matches = result.get('total_matches', 0)
        showing = result.get('showing', len(agents))

        if agents:
            console.print(f"[bold magenta]Agent Search Results (showing {showing} of {total_matches} matches)[/bold magenta]\n")

            table = Table(show_header=True, header_style="bold magenta")
            table.add_column("Agent Name", style="cyan", width=20)
            table.add_column("Agent ID", style="blue", width=50)
            table.add_column("Description", style="green", width=40)
            table.add_column("Tags", style="yellow", width=20)
            # table.add_column("Version", style="magenta", width=10)

            for agent in agents:
                agent_name = agent.get('agent_name', 'N/A')
                agent_id = agent.get('agent_id', 'N/A')
                description = agent.get('description', 'N/A')
                tags = agent.get('tags', [])
                # version = agent.get('version', 'N/A')

                # # Truncate long descriptions
                # if len(description) > 50:
                #     description = description[:47] + "..."

                # Format tags
                tags_str = ', '.join(tags[:3]) if tags else 'N/A'
                # if len(tags) > 3:
                #     tags_str += "..."

                table.add_row(agent_name, agent_id, description, tags_str)

            console.print(table)
        else:
            console.print(f"[yellow]No agents found matching '{query}'[/yellow]")

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")
        raise typer.Exit(1)
