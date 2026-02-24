"""Modern observability command group."""

import typer

# Create Observability command group
observability_app = typer.Typer(help="Observability and performance monitoring")


@observability_app.command(name="sessions")
def sessions(
    agent_id: str = typer.Argument(None, help="Agent ID to filter sessions (optional)"),
    days: int = typer.Option(7, "--days", "-d", help="Number of days to look back (default: 7)"),
    limit: int = typer.Option(20, "--limit", "-l", help="Number of sessions to show (default: 20)"),
    format_type: str = typer.Option(
        "table", "--format", "-f", help="Output format: table, json, summary"
    ),
):
    """Get observability sessions for agents."""
    from commands.observability import sessions_command
    sessions_command(agent_id, days, limit, format_type)


@observability_app.command(name="session")
def session_details(
    session_id: str = typer.Argument(..., help="Session ID to get details for"),
    format_type: str = typer.Option(
        "detailed", "--format", "-f", help="Output format: detailed, json, traces"
    ),
):
    """Get detailed information about a specific session."""
    from commands.observability import session_details_command
    session_details_command(session_id, format_type)


@observability_app.command(name="trace")
def trace_details(
    project_id: str = typer.Argument(..., help="Project ID"),
    trace_id: str = typer.Argument(..., help="Trace ID to get details for"),
    format_type: str = typer.Option(
        "tree", "--format", "-f", help="Output format: tree, json, spans"
    ),
):
    """Get detailed trace information with nested spans."""
    from commands.observability import trace_details_command
    trace_details_command(project_id, trace_id, format_type)


@observability_app.command(name="span")
def span_details(
    span_id: str = typer.Argument(..., help="Span ID to get details for"),
    format_type: str = typer.Option(
        "detailed", "--format", "-f", help="Output format: detailed, json"
    ),
):
    """Get detailed information about a specific span."""
    from commands.observability import span_details_command
    span_details_command(span_id, format_type)


@observability_app.command(name="stats")
def agent_stats(
    agent_id: str = typer.Argument(..., help="Agent ID to get stats for"),
    days: int = typer.Option(7, "--days", "-d", help="Number of days to analyze (default: 7)"),
    format_type: str = typer.Option(
        "summary", "--format", "-f", help="Output format: summary, json"
    ),
):
    """Get performance statistics for an agent."""
    from commands.observability import agent_stats_command
    agent_stats_command(agent_id, days, format_type)