"""
Chat History/Session commands for Nasiko CLI.
"""

from typing import Optional

import typer
from rich.console import Console
from rich.panel import Panel
from rich.table import Table

from core.settings import APIEndpoints
from core.api_client import get_api_client

console = Console()


def create_session(agent_name: Optional[str] = None):
    """Create a new chat session."""

    try:
        client = get_api_client()
        
        # Send agent_name wrapped in proper request body structure
        if agent_name:
            response = client.post(APIEndpoints.CHAT_SESSION, data={"agent_id": agent_name})
        else:
            response = client.post(APIEndpoints.CHAT_SESSION, data={})
        
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)
            
        data = result.get("data", result)

        session_id = data.get("session_id")
        console.print(f"[green]✅ Session created successfully[/green]")
        console.print(f"[cyan]Session ID: {session_id}[/cyan]")
        if data.get("created_at"):
            console.print(f"[cyan]Created: {data['created_at']}[/cyan]")
        if data.get("title"):
            console.print(f"[cyan]Title: {data['title']}[/cyan]")

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")
        raise typer.Exit(1)


def list_sessions(limit: int = 10, cursor: Optional[str] = None, direction: str = "after"):
    """Get paginated list of chat sessions."""

    try:
        client = get_api_client()
        params = {"limit": limit, "direction": direction}
        if cursor:
            params["cursor"] = cursor

        response = client.get(APIEndpoints.CHAT_SESSION_LIST, params=params)
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)
            
        data = result.get("data", result)

        if data:
            console.print(f"[bold magenta]Chat Sessions ({len(data)} found)[/bold magenta]\n")

            table = Table(show_header=True, header_style="bold magenta")
            table.add_column("Session ID", style="cyan", width=40)
            table.add_column("Title", style="yellow", width=30)

            for session in data:
                table.add_row(
                    session.get("session_id", "N/A"),
                    session.get("title", "N/A"),
                )

            console.print(table)

            # # Show pagination info
            # if data.get("has_more"):
            #     console.print(
            #         f"\n[yellow]Has more results. Next cursor: {data.get('next_cursor')}[/yellow]"
            #     )
        else:
            console.print("[yellow]No sessions found[/yellow]")

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")
        raise typer.Exit(1)


def get_chat_history(
    session_id: str, limit: int = 50, cursor: Optional[str] = None, direction: str = "after"
):
    """Get chat history for a specific session."""

    try:
        client = get_api_client()
        params = {"limit": limit, "direction": direction}
        if cursor:
            params["cursor"] = cursor

        url = APIEndpoints.CHAT_SESSION_BY_ID.format(session_id=session_id)
        response = client.get(url, params=params)
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)
            
        data = result.get("data", result)

        if data:
            console.print(f"[bold magenta]Chat History - Session {session_id}[/bold magenta]")
            console.print(f"[cyan]({len(data)} messages)[/cyan]\n")

            for msg in data:
                role = msg.get("role", "unknown")
                content = msg.get("content", "")
                timestamp = msg.get("timestamp", "N/A")

                # Color code by role
                if role == "user":
                    role_color = "blue"
                elif role == "assistant":
                    role_color = "green"
                else:
                    role_color = "yellow"

                msg_text = f"[bold {role_color}][{timestamp}] {role}:[/bold {role_color}]\n"

                # # Truncate long messages
                # if len(content) > 200:
                #     msg_text += f"{content[:200]}...\n"
                # else:
                #     msg_text += f"{content}\n"
                msg_text += f"{content}\n"
                console.print(Panel(msg_text.strip(), border_style=role_color))

            # # Show pagination info
            # if data.get("has_more"):
            #     console.print(
            #         f"\n[yellow]Has more messages. Next cursor: {data.get('next_cursor')}[/yellow]"
            #     )
        else:
            console.print("[yellow]No messages found in this session[/yellow]")

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")
        raise typer.Exit(1)


def delete_session(session_id: str):
    """Delete a chat session."""

    console.print(f"[yellow]Are you sure you want to delete session '{session_id}'?[/yellow]")
    console.print("[yellow]This action cannot be undone.[/yellow]")

    confirm = typer.confirm("Continue with deletion?")
    if not confirm:
        console.print("[blue]Deletion cancelled[/blue]")
        return

    try:
        client = get_api_client()
        url = APIEndpoints.CHAT_SESSION_BY_ID.format(session_id=session_id)
        response = client.delete(url)
        result = client.handle_response(response, success_message=f"Successfully deleted session: {session_id}")
        if result is None:
            raise typer.Exit(1)

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")
        raise typer.Exit(1)
