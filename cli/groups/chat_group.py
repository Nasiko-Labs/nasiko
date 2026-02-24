"""
Chat command group.
"""

from typing import Optional
import typer

# Create Chat command group
chat_app = typer.Typer(help="Chat sessions and conversation history")


@chat_app.command(name="create-session")
def chat_session_create(
    agent_name: Optional[str] = typer.Option(
        None, "--agent", "-a", help="Agent name to associate with session"
    ),
):
    """Create a new chat session."""
    from commands.chat_history import create_session
    create_session(agent_name)


@chat_app.command(name="list-sessions")
def chat_sessions(
    limit: int = typer.Option(10, "--limit", "-l", help="Number of sessions per page (1-20)"),
    cursor: Optional[str] = typer.Option(None, "--cursor", help="Cursor for pagination"),
    direction: str = typer.Option(
        "after", "--direction", help="Direction for pagination (before/after)"
    ),
):
    """List chat sessions."""
    from commands.chat_history import list_sessions
    list_sessions(limit, cursor, direction)


@chat_app.command(name="history")
def chat_history(
    session_id: str = typer.Argument(..., help="Session ID"),
    limit: int = typer.Option(50, "--limit", "-l", help="Number of messages per page (1-100)"),
    cursor: Optional[str] = typer.Option(None, "--cursor", help="Cursor for pagination"),
    direction: str = typer.Option(
        "after", "--direction", help="Direction for pagination (before/after)"
    ),
):
    """Get chat history for a specific session."""
    from commands.chat_history import get_chat_history
    get_chat_history(session_id, limit, cursor, direction)


@chat_app.command(name="delete-session")
def chat_session_delete(
    session_id: str = typer.Argument(..., help="Session ID to delete"),
):
    """Delete a chat session."""
    from commands.chat_history import delete_session
    delete_session(session_id)


@chat_app.command(name="send")
def chat_send(
    url: str = typer.Option(..., "--url", "-u", prompt="Agent URL", help="Agent URL to send message to"),
    session_id: str = typer.Option(..., "--session-id", "-s", prompt="Session ID", help="Session ID for the conversation"),
    message: str = typer.Option(..., "--message", "-m", prompt="Message", help="Message to send to the agent"),
):
    """Send a message to an agent and get the response."""
    from commands.chat_send import send_message_command
    send_message_command(url, message, session_id)