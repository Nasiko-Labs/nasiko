"""
Modern observability commands for the Nasiko CLI.
"""

import typer
import requests
from typing import Optional, List, Dict, Any
from rich.console import Console
from rich.table import Table
from rich.panel import Panel
from rich.json import JSON
from rich.progress import Progress, SpinnerColumn, TextColumn
from rich.columns import Columns
from rich.box import ROUNDED
from rich.align import Align
from rich.text import Text
from datetime import datetime, timedelta
from core.settings import APIEndpoints
from auth.auth_manager import AuthManager

console = Console()

def format_datetime(dt_str):
    """Format datetime string for display"""
    if not dt_str:
        return "N/A"
    try:
        for fmt in ["%Y-%m-%dT%H:%M:%S.%fZ", "%Y-%m-%dT%H:%M:%SZ", "%Y-%m-%dT%H:%M:%S"]:
            try:
                dt = datetime.strptime(dt_str, fmt)
                return dt.strftime("%m/%d %H:%M")
            except ValueError:
                continue
        return dt_str[:16] if len(dt_str) > 16 else dt_str
    except Exception:
        return dt_str

def format_duration(duration_ms):
    """Format duration in milliseconds to readable format"""
    if not duration_ms:
        return "N/A"
    
    try:
        duration_ms = float(duration_ms)
        if duration_ms < 1000:
            return f"{duration_ms:.0f}ms"
        elif duration_ms < 60000:
            return f"{duration_ms/1000:.1f}s"
        else:
            minutes = duration_ms / 60000
            return f"{minutes:.1f}m"
    except (ValueError, TypeError):
        return str(duration_ms)

def format_cost(cost):
    """Format cost for display"""
    if not cost:
        return "$0.00"
    try:
        cost_val = float(cost)
        if cost_val < 0.01:
            return f"${cost_val:.4f}"
        else:
            return f"${cost_val:.2f}"
    except (ValueError, TypeError):
        return str(cost)

def format_tokens(tokens):
    """Format token count for display"""
    if not tokens:
        return "0"
    try:
        tokens_val = int(tokens)
        if tokens_val >= 1000:
            return f"{tokens_val/1000:.1f}K"
        else:
            return str(tokens_val)
    except (ValueError, TypeError):
        return str(tokens)

def get_status_color(status):
    """Get color for status display"""
    status_colors = {
        "ok": "green", "success": "green", "completed": "green",
        "error": "red", "failed": "red", "timeout": "red",
        "cancelled": "yellow", "pending": "blue", "running": "cyan",
        "unknown": "white"
    }
    return status_colors.get(str(status).lower(), "white")

def get_auth_headers():
    """Get authentication headers"""
    auth_manager = AuthManager()
    headers = auth_manager.get_auth_headers()
    if not headers:
        console.print("[red]Error: Not authenticated. Please run 'nasiko login' first.[/red]")
        raise typer.Exit(1)
    return headers

def sessions_command(
    agent_id: Optional[str] = typer.Argument(None, help="Agent ID to filter sessions (optional)"),
    days: int = typer.Option(7, "--days", "-d", help="Number of days to look back (default: 7)"),
    limit: int = typer.Option(20, "--limit", "-l", help="Number of sessions to show (default: 20)"),
    format_type: str = typer.Option("table", "--format", "-f", help="Output format: table, json, summary"),
):
    """Get observability sessions for agents"""
    
    try:
        headers = get_auth_headers()
        
        # Calculate start time for filtering
        start_time = (datetime.utcnow() - timedelta(days=days)).isoformat() + "Z"
        
        params = {"start_time": start_time}
        
        with Progress(
            SpinnerColumn(), 
            TextColumn("[progress.description]{task.description}"),
            console=console
        ) as progress:
            task = progress.add_task("Fetching observability sessions...", total=None)
            response = requests.get(APIEndpoints.OBSERVABILITY_SESSIONS, headers=headers, params=params, timeout=30)
            progress.remove_task(task)
        
        if response.status_code == 401:
            console.print("[red]Error: Authentication failed. Please run 'nasiko auth login'[/red]")
            return
        elif response.status_code == 404:
            console.print("[yellow]No sessions found[/yellow]")
            return
        
        response.raise_for_status()
        data = response.json()
        
        sessions_data = data.get('data', {})
        sessions = sessions_data.get('sessions', [])
        
        if not sessions:
            console.print(f"[yellow]No sessions found in the last {days} days[/yellow]")
            return
        
        # Filter by agent_id if specified
        if agent_id:
            sessions = [s for s in sessions if s.get('agent_id') == agent_id]
            if not sessions:
                console.print(f"[yellow]No sessions found for agent '{agent_id}' in the last {days} days[/yellow]")
                return
        
        # Limit sessions
        sessions = sessions[:limit]
        
        total_agents = sessions_data.get('total_agents', 0)
        successful_agents = sessions_data.get('successful_agents', 0)
        
        # Display header with stats
        header_text = f"[bold cyan]Observability Sessions[/bold cyan]"
        if agent_id:
            header_text += f" for [bold yellow]{agent_id}[/bold yellow]"
        
        stats_text = f"[dim]Agents: {successful_agents}/{total_agents} | Sessions: {len(sessions)} | Last {days} days[/dim]"
        
        console.print(f"{header_text}\n{stats_text}\n")
        
        if format_type == "json":
            console.print(JSON.from_data(sessions))
        elif format_type == "summary":
            display_sessions_summary(sessions, days)
        else:
            display_sessions_table(sessions)
        
    except requests.exceptions.ConnectionError:
        console.print("[red]Error: Could not connect to Nasiko API. Make sure the server is running.[/red]")
    except requests.exceptions.Timeout:
        console.print("[red]Error: Request timed out. The observability service might be busy.[/red]")
    except requests.exceptions.HTTPError as e:
        if e.response.status_code == 400:
            console.print(f"[red]Error: Invalid request - {e.response.text}[/red]")
        else:
            console.print(f"[red]Error: HTTP {e.response.status_code} - {e.response.text}[/red]")
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")

def session_details_command(
    session_id: str = typer.Argument(..., help="Session ID to get details for"),
    format_type: str = typer.Option("detailed", "--format", "-f", help="Output format: detailed, json, traces"),
):
    """Get detailed information about a specific session"""
    
    try:
        headers = get_auth_headers()
        
        url = APIEndpoints.OBSERVABILITY_SESSION_DETAILS.format(session_id=session_id)
        
        with Progress(
            SpinnerColumn(),
            TextColumn("[progress.description]{task.description}"),
            console=console
        ) as progress:
            task = progress.add_task(f"Fetching session {session_id[:8]}...", total=None)
            response = requests.get(url, headers=headers, timeout=30)
            progress.remove_task(task)
        
        if response.status_code == 404:
            console.print(f"[red]Session '{session_id}' not found[/red]")
            return
        
        response.raise_for_status()
        data = response.json()
        
        session = data.get('data', {}).get('session', {})
        
        if not session:
            console.print(f"[yellow]No session data found for '{session_id}'[/yellow]")
            return
        
        if format_type == "json":
            console.print(JSON.from_data(session))
        elif format_type == "traces":
            display_session_traces(session)
        else:
            display_session_details(session)
        
    except requests.exceptions.ConnectionError:
        console.print("[red]Error: Could not connect to Nasiko API. Make sure the server is running.[/red]")
    except requests.exceptions.Timeout:
        console.print("[red]Error: Request timed out. The observability service might be busy.[/red]")
    except requests.exceptions.HTTPError as e:
        console.print(f"[red]Error: HTTP {e.response.status_code} - {e.response.text}[/red]")
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")

def trace_details_command(
    project_id: str = typer.Argument(..., help="Project ID"),
    trace_id: str = typer.Argument(..., help="Trace ID to get details for"),
    format_type: str = typer.Option("tree", "--format", "-f", help="Output format: tree, json, spans"),
):
    """Get detailed trace information with nested spans"""
    
    try:
        headers = get_auth_headers()
        
        url = APIEndpoints.OBSERVABILITY_TRACE_DETAILS.format(project_id=project_id, trace_id=trace_id)
        
        with Progress(
            SpinnerColumn(),
            TextColumn("[progress.description]{task.description}"),
            console=console
        ) as progress:
            task = progress.add_task(f"Fetching trace {trace_id[:8]}...", total=None)
            response = requests.get(url, headers=headers, timeout=30)
            progress.remove_task(task)
        
        if response.status_code == 404:
            console.print(f"[red]Trace '{trace_id}' not found in project '{project_id}'[/red]")
            return
        
        response.raise_for_status()
        data = response.json()
        
        trace = data.get('data', {}).get('trace', {})
        
        if not trace:
            console.print(f"[yellow]No trace data found for '{trace_id}'[/yellow]")
            return
        
        if format_type == "json":
            console.print(JSON.from_data(trace))
        elif format_type == "spans":
            display_trace_spans_flat(trace)
        else:
            display_trace_tree(trace)
        
    except requests.exceptions.ConnectionError:
        console.print("[red]Error: Could not connect to Nasiko API. Make sure the server is running.[/red]")
    except requests.exceptions.Timeout:
        console.print("[red]Error: Request timed out. The observability service might be busy.[/red]")
    except requests.exceptions.HTTPError as e:
        console.print(f"[red]Error: HTTP {e.response.status_code} - {e.response.text}[/red]")
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")

def span_details_command(
    span_id: str = typer.Argument(..., help="Span ID to get details for"),
    format_type: str = typer.Option("detailed", "--format", "-f", help="Output format: detailed, json"),
):
    """Get detailed information about a specific span"""
    
    try:
        headers = get_auth_headers()
        
        url = APIEndpoints.OBSERVABILITY_SPAN_DETAILS.format(span_id=span_id)
        
        with Progress(
            SpinnerColumn(),
            TextColumn("[progress.description]{task.description}"),
            console=console
        ) as progress:
            task = progress.add_task(f"Fetching span {span_id[:8]}...", total=None)
            response = requests.get(url, headers=headers, timeout=30)
            progress.remove_task(task)
        
        if response.status_code == 404:
            console.print(f"[red]Span '{span_id}' not found[/red]")
            return
        
        response.raise_for_status()
        data = response.json()
        
        span = data.get('data', {}).get('span', {})
        
        if not span:
            console.print(f"[yellow]No span data found for '{span_id}'[/yellow]")
            return
        
        if format_type == "json":
            console.print(JSON.from_data(span))
        else:
            display_span_details(span)
        
    except requests.exceptions.ConnectionError:
        console.print("[red]Error: Could not connect to Nasiko API. Make sure the server is running.[/red]")
    except requests.exceptions.Timeout:
        console.print("[red]Error: Request timed out. The observability service might be busy.[/red]")
    except requests.exceptions.HTTPError as e:
        console.print(f"[red]Error: HTTP {e.response.status_code} - {e.response.text}[/red]")
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")

def agent_stats_command(
    agent_id: str = typer.Argument(..., help="Agent ID to get stats for"),
    days: int = typer.Option(7, "--days", "-d", help="Number of days to analyze (default: 7)"),
    format_type: str = typer.Option("summary", "--format", "-f", help="Output format: summary, json"),
):
    """Get performance statistics for an agent"""
    
    try:
        headers = get_auth_headers()
        
        start_time = (datetime.utcnow() - timedelta(days=days)).isoformat() + "Z"
        
        url = APIEndpoints.OBSERVABILITY_AGENT_STATS.format(agent_id=agent_id)
        params = {"start_time": start_time}
        
        with Progress(
            SpinnerColumn(),
            TextColumn("[progress.description]{task.description}"),
            console=console
        ) as progress:
            task = progress.add_task(f"Fetching stats for {agent_id}...", total=None)
            response = requests.get(url, headers=headers, params=params, timeout=30)
            progress.remove_task(task)
        
        if response.status_code == 404:
            console.print(f"[red]Agent '{agent_id}' not found[/red]")
            return
        
        response.raise_for_status()
        data = response.json()
        
        project_stats = data.get('data', {}).get('project', {})
        
        if not project_stats:
            console.print(f"[yellow]No stats found for agent '{agent_id}'[/yellow]")
            return
        
        if format_type == "json":
            console.print(JSON.from_data(project_stats))
        else:
            display_agent_stats(agent_id, project_stats, days)
        
    except requests.exceptions.ConnectionError:
        console.print("[red]Error: Could not connect to Nasiko API. Make sure the server is running.[/red]")
    except requests.exceptions.Timeout:
        console.print("[red]Error: Request timed out. The observability service might be busy.[/red]")
    except requests.exceptions.HTTPError as e:
        console.print(f"[red]Error: HTTP {e.response.status_code} - {e.response.text}[/red]")
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")

def display_sessions_table(sessions: List[Dict[str, Any]]):
    """Display sessions in a clean table format"""
    
    table = Table(show_header=True, header_style="bold cyan", box=ROUNDED)
    table.add_column("Agent", style="yellow", width=15)
    table.add_column("Session", style="cyan", width=36) 
    table.add_column("Traces", style="magenta", width=8)
    table.add_column("Tokens", style="blue", width=8)
    table.add_column("Cost", style="green", width=8)
    table.add_column("Latency", style="white", width=8)
    table.add_column("Started", style="dim", width=18)
    
    for session in sessions:
        agent_id = session.get('agent_id', 'Unknown')[:14] + "..." if len(session.get('agent_id', '')) > 14 else session.get('agent_id', 'Unknown')
        session_id = session.get('session_id', 'N/A')
        num_traces = str(session.get('num_traces', 0))
        
        # Token usage
        token_usage = session.get('token_usage', {})
        total_tokens = format_tokens(token_usage.get('total', 0))
        
        # Cost
        cost_summary = session.get('cost_summary', {})
        total_cost = format_cost(cost_summary.get('total', {}).get('cost', 0))
        
        # Latency
        latency = format_duration(session.get('trace_latency_ms_p50'))
        
        # Start time
        start_time = format_datetime(session.get('start_time'))
        
        table.add_row(agent_id, session_id, num_traces, total_tokens, total_cost, latency, start_time)
    
    console.print(table)

def display_sessions_summary(sessions: List[Dict[str, Any]], days: int):
    """Display a summary of sessions with key metrics"""
    
    # Calculate aggregated metrics
    total_sessions = len(sessions)
    total_traces = sum(session.get('num_traces', 0) for session in sessions)
    total_tokens = sum(session.get('token_usage', {}).get('total', 0) for session in sessions)
    total_cost = sum(session.get('cost_summary', {}).get('total', {}).get('cost', 0) for session in sessions)
    
    # Agent distribution
    agents = {}
    for session in sessions:
        agent_id = session.get('agent_id', 'Unknown')
        agents[agent_id] = agents.get(agent_id, 0) + 1
    
    # Summary metrics
    metrics_info = f"""[bold]Total Sessions:[/bold] {total_sessions}
[bold]Total Traces:[/bold] {total_traces:,}
[bold]Total Tokens:[/bold] {format_tokens(total_tokens)}
[bold]Total Cost:[/bold] {format_cost(total_cost)}
[bold]Period:[/bold] Last {days} days"""
    
    console.print(Panel(metrics_info, title="📊 Session Summary", border_style="cyan"))
    
    # Agent breakdown
    if agents:
        agent_info = ""
        sorted_agents = sorted(agents.items(), key=lambda x: x[1], reverse=True)
        for agent, count in sorted_agents[:5]:
            percentage = (count / total_sessions) * 100
            agent_short = agent[:20] + "..." if len(agent) > 20 else agent
            agent_info += f"{agent_short}: {count} sessions ({percentage:.1f}%)\n"
        
        console.print(Panel(agent_info.strip(), title="🤖 Top Agents", border_style="yellow"))

def display_session_details(session: Dict[str, Any]):
    """Display detailed session information"""
    
    session_id = session.get('session_id', 'N/A')
    console.print(f"[bold cyan]Session Details: {session_id}[/bold cyan]\n")
    
    # Session overview
    overview_info = f"""[bold]Session ID:[/bold] {session_id}
[bold]Total Traces:[/bold] {session.get('num_traces', 0)}
[bold]P50 Latency:[/bold] {format_duration(session.get('latency_p50'))}"""
    
    # Token usage
    token_usage = session.get('token_usage', {})
    if token_usage.get('total'):
        overview_info += f"\n[bold]Token Usage:[/bold] {format_tokens(token_usage['total'])}"
    
    # Cost summary
    cost_summary = session.get('cost_summary', {})
    if cost_summary:
        total_cost = cost_summary.get('total', {}).get('cost', 0)
        prompt_cost = cost_summary.get('prompt', {}).get('cost', 0)
        completion_cost = cost_summary.get('completion', {}).get('cost', 0)
        
        if total_cost:
            overview_info += f"\n[bold]Total Cost:[/bold] {format_cost(total_cost)}"
            overview_info += f"\n[bold]Prompt Cost:[/bold] {format_cost(prompt_cost)} | [bold]Completion Cost:[/bold] {format_cost(completion_cost)}"
    
    console.print(Panel(overview_info, title="📊 Overview", border_style="blue"))
    
    # Traces table
    traces = session.get('traces', [])
    if traces:
        console.print(f"\n[bold]Traces ({len(traces)}):[/bold]")
        display_traces_table(traces, session_id)

def display_session_traces(session: Dict[str, Any]):
    """Display just the traces from a session"""
    traces = session.get('traces', [])
    if not traces:
        console.print("[yellow]No traces found in this session[/yellow]")
        return
    
    session_id = session.get('session_id', 'N/A')
    console.print(f"[bold cyan]Session Traces: {session_id}[/bold cyan]\n")
    display_traces_table(traces, session_id)

def display_traces_table(traces: List[Dict[str, Any]], session_id: str = None):
    """Display traces in a clean table format"""
    
    table = Table(show_header=True, header_style="bold magenta", box=ROUNDED)
    table.add_column("Project ID", style="blue", width=16)  
    table.add_column("Trace ID", style="cyan", width=36)  # Full UUID width
    table.add_column("Tokens", style="yellow", width=8)
    table.add_column("Cost", style="green", width=8)
    table.add_column("Latency", style="white", width=8)
    table.add_column("Time", style="dim", width=18)
    
    for trace in traces:
        # Get project ID from the nested structure and trace ID
        root_span = trace.get('root_span', {})
        project_data = root_span.get('project', {})
        project_id = project_data.get('id', 'N/A')
        trace_id = trace.get('trace_id', 'N/A')  # Show full trace ID
        
        # Get root span data for metrics
        root_span = trace.get('root_span', {})
        
        # Metrics from root span
        tokens = format_tokens(root_span.get('cumulative_token_count_total', 0))
        latency = format_duration(root_span.get('latency_ms'))
        start_time = format_datetime(root_span.get('start_time'))
        
        # Cost from trace
        trace_data = root_span.get('trace', {})
        cost_summary = trace_data.get('cost_summary', {})
        cost = format_cost(cost_summary.get('total', {}).get('cost', 0))
        
        table.add_row(project_id, trace_id, tokens, cost, latency, start_time)
    
    console.print(table)

def fetch_session_history(session_id: str) -> Optional[Dict[str, Any]]:
    """Fetch session details for enhanced trace data - similar to chat history approach"""
    try:
        headers = get_auth_headers()
        
        # Use the session details endpoint - this should contain the traces with full input/output
        url = APIEndpoints.OBSERVABILITY_SESSION_DETAILS.format(session_id=session_id)
        response = requests.get(url, headers=headers, timeout=30)
        
        if response.status_code == 200:
            data = response.json()
            session_data = data.get('data', {}).get('session', {})
            # Return the session data which should include traces with enhanced details
            return session_data
        
    except Exception:
        pass  # Silently fail, we'll use fallback data
    
    return None

def get_enhanced_trace_io(trace: Dict[str, Any], trace_id: str, session_history: Optional[Dict[str, Any]]) -> tuple[str, str]:
    """Get enhanced input/output data for a trace from session details"""
    
    # Try to get data from session history first (like chat history does)
    if session_history and session_history.get('traces'):
        for hist_trace in session_history['traces']:
            if hist_trace.get('trace_id') == trace_id:
                # Get input/output from the session details
                hist_input = ''
                hist_output = ''
                
                # Check root span in session history
                root_span = hist_trace.get('root_span', {})
                if root_span:
                    input_obj = root_span.get('input', {})
                    output_obj = root_span.get('output', {})
                    
                    hist_input = input_obj.get('value', '') if input_obj else ''
                    hist_output = output_obj.get('value', '') if output_obj else ''
                
                # If we found data from session history, use it
                if hist_input or hist_output:
                    input_display = format_io_for_table(hist_input, 28)
                    output_display = format_io_for_table(hist_output, 28)
                    return input_display, output_display
    
    # Fallback to current trace data
    root_span = trace.get('root_span', {})
    fallback_input = ''
    fallback_output = ''
    
    if root_span:
        input_obj = root_span.get('input', {})
        output_obj = root_span.get('output', {})
        
        fallback_input = input_obj.get('value', '') if input_obj else ''
        fallback_output = output_obj.get('value', '') if output_obj else ''
    
    input_display = format_io_for_table(fallback_input, 28)
    output_display = format_io_for_table(fallback_output, 28)
    
    return input_display, output_display

def format_io_for_table(text: str, max_width: int) -> str:
    """Format input/output text for table display"""
    if not text or text == 'N/A':
        return 'N/A'
    
    # Clean up the text - remove excessive whitespace and newlines
    cleaned = ' '.join(str(text).strip().split())
    
    # Truncate if too long
    if len(cleaned) > max_width:
        return cleaned[:max_width-3] + "..."
    
    return cleaned

def display_trace_tree(trace: Dict[str, Any]):
    """Display trace with nested spans in a tree format"""
    
    console.print(f"[bold cyan]Trace Tree[/bold cyan]\n")
    
    # Trace overview
    trace_info = f"""[bold]Trace ID:[/bold] {trace.get('id', 'N/A')}
[bold]Project Session:[/bold] {trace.get('project_session_id', 'N/A')}
[bold]Total Spans:[/bold] {trace.get('num_spans', 0)}
[bold]Latency:[/bold] {format_duration(trace.get('latency_ms'))}"""
    
    cost_summary = trace.get('cost_summary', {})
    if cost_summary:
        total_cost = format_cost(cost_summary.get('total', {}).get('cost', 0))
        trace_info += f"\n[bold]Total Cost:[/bold] {total_cost}"
    
    console.print(Panel(trace_info, title="🔍 Trace Overview", border_style="blue"))
    
    # Span tree
    spans = trace.get('spans', [])
    if spans:
        console.print(f"\n[bold]Span Tree:[/bold]")
        display_spans_recursive(spans, depth=0)

def display_spans_recursive(spans: List[Dict[str, Any]], depth: int = 0):
    """Recursively display spans in tree format with span IDs"""
    
    for span in spans:
        indent = "  " * depth
        status_color = get_status_color(span.get('status_code', 'unknown'))
        
        # Get both span ID and ID for complete reference
        span_id = span.get('span_id', 'N/A')
        span_db_id = span.get('id', 'N/A')
        
        # Format IDs for display
        span_id_short = span_id[:8] + "..." if len(str(span_id)) > 8 else str(span_id)
        span_db_id_short = span_db_id[:12] if len(str(span_db_id)) > 12 else str(span_db_id)
        
        # Show both IDs: db_id | span_id
        id_display = f"[dim]({span_db_id_short}|{span_id_short})[/dim]"
        
        span_info = f"{indent}├─ {id_display} [bold]{span.get('name', 'Unknown')}[/bold]"
        span_info += f" [{status_color}]{span.get('status_code', 'unknown')}[/{status_color}]"
        span_info += f" {format_duration(span.get('latency_ms'))}"
        
        if span.get('token_count_total'):
            span_info += f" {format_tokens(span.get('token_count_total'))}tok"
        
        console.print(span_info)
        
        # Display children recursively
        children = span.get('children', [])
        if children:
            display_spans_recursive(children, depth + 1)

def display_trace_spans_flat(trace: Dict[str, Any]):
    """Display trace spans in a flat table format"""
    
    # Flatten all spans from the tree
    def flatten_spans(spans, all_spans=None):
        if all_spans is None:
            all_spans = []
        for span in spans:
            all_spans.append(span)
            if span.get('children'):
                flatten_spans(span['children'], all_spans)
        return all_spans
    
    all_spans = flatten_spans(trace.get('spans', []))
    
    if not all_spans:
        console.print("[yellow]No spans found in this trace[/yellow]")
        return
    
    table = Table(show_header=True, header_style="bold magenta", box=ROUNDED)
    table.add_column("Span ID", style="cyan", width=10)
    table.add_column("Name", style="blue", width=20)
    table.add_column("Kind", style="yellow", width=12)
    table.add_column("Status", style="green", width=10)
    table.add_column("Latency", style="white", width=10)
    table.add_column("Tokens", style="magenta", width=8)
    table.add_column("Parent", style="dim", width=10)
    
    for span in all_spans:
        span_id = span.get('span_id', 'N/A')[:8] + "..." if len(span.get('span_id', '')) > 8 else span.get('span_id', 'N/A')
        name = span.get('name', 'Unknown')[:18] + "..." if len(span.get('name', '')) > 18 else span.get('name', 'Unknown')
        kind = span.get('span_kind', 'N/A')
        status = span.get('status_code', 'unknown')
        status_color = get_status_color(status)
        colored_status = f"[{status_color}]{status}[/{status_color}]"
        latency = format_duration(span.get('latency_ms'))
        tokens = format_tokens(span.get('token_count_total', 0))
        parent_id = span.get('parent_id', '')[:8] if span.get('parent_id') else 'root'
        
        table.add_row(span_id, name, kind, colored_status, latency, tokens, parent_id)
    
    console.print(table)

def display_span_details(span: Dict[str, Any]):
    """Display detailed span information"""
    
    span_id = span.get('span_id', span.get('id', 'N/A'))
    console.print(f"[bold cyan]Span Details: {span_id}[/bold cyan]\n")
    
    # Span overview
    span_name = span.get('name', 'Unknown')
    span_kind = span.get('span_kind', 'N/A')
    status_code = span.get('status_code', 'unknown')
    status_color = get_status_color(status_code)
    
    overview_info = f"""[bold]Span ID:[/bold] {span_id}
[bold]Name:[/bold] {span_name}
[bold]Kind:[/bold] {span_kind}
[bold]Status:[/bold] [{status_color}]{status_code}[/{status_color}]
[bold]Latency:[/bold] {format_duration(span.get('latency_ms'))}
[bold]Start Time:[/bold] {format_datetime(span.get('start_time'))}
[bold]End Time:[/bold] {format_datetime(span.get('end_time'))}"""
    
    # Token usage
    if span.get('token_count_total'):
        overview_info += f"\n[bold]Token Usage:[/bold] {format_tokens(span.get('token_count_total'))}"
    
    # Parent/Children
    parent_id = span.get('parent_id')
    if parent_id:
        overview_info += f"\n[bold]Parent Span:[/bold] {parent_id[:16]}..."
    
    console.print(Panel(overview_info, title="🔍 Span Overview", border_style="blue"))
    
    # Attributes
    attributes = span.get('attributes')
    if attributes:
        try:
            if isinstance(attributes, str):
                import json
                attributes_data = json.loads(attributes)
            else:
                attributes_data = attributes
            
            if attributes_data:
                console.print(f"\n[bold]Attributes:[/bold]")
                console.print(JSON.from_data(attributes_data))
        except Exception:
            console.print(f"\n[bold]Attributes:[/bold] {attributes}")
    
    # Input/Output if available
    input_data = span.get('input')
    output_data = span.get('output')
    
    if input_data or output_data:
        io_info = ""
        if input_data:
            input_text = input_data.get('value', str(input_data)) if isinstance(input_data, dict) else str(input_data)
            io_info += f"[bold]Input:[/bold]\n{input_text[:500]}{'...' if len(str(input_text)) > 500 else ''}\n\n"
        
        if output_data:
            output_text = output_data.get('value', str(output_data)) if isinstance(output_data, dict) else str(output_data)
            io_info += f"[bold]Output:[/bold]\n{output_text[:500]}{'...' if len(str(output_text)) > 500 else ''}"
        
        console.print(Panel(io_info.strip(), title="📝 Input/Output", border_style="green"))
    
    # Annotations
    annotations = span.get('span_annotations', [])
    if annotations:
        console.print(f"\n[bold]Annotations ({len(annotations)}):[/bold]")
        for annotation in annotations[:3]:  # Show first 3
            annotation_name = annotation.get('name', 'Unknown')
            annotation_value = annotation.get('value', 'N/A')
            console.print(f"  • [yellow]{annotation_name}:[/yellow] {annotation_value}")
        
        if len(annotations) > 3:
            console.print(f"  ... and {len(annotations) - 3} more annotations")

def display_agent_stats(agent_id: str, stats: Dict[str, Any], days: int):
    """Display agent performance statistics"""
    
    console.print(f"[bold cyan]Agent Statistics: {agent_id}[/bold cyan]")
    console.print(f"[dim]Last {days} days[/dim]\n")
    
    # Performance metrics
    trace_count = stats.get('trace_count', 0)
    latency_p50 = format_duration(stats.get('latency_ms_p50'))
    latency_p99 = format_duration(stats.get('latency_ms_p99'))
    
    perf_info = f"""[bold]Total Traces:[/bold] {trace_count:,}
[bold]P50 Latency:[/bold] {latency_p50}
[bold]P99 Latency:[/bold] {latency_p99}"""
    
    console.print(Panel(perf_info, title="⚡ Performance", border_style="green"))
    
    # Cost metrics
    cost_summary = stats.get('cost_summary', {})
    if cost_summary:
        total_cost = format_cost(cost_summary.get('total', {}).get('cost', 0))
        prompt_cost = format_cost(cost_summary.get('prompt', {}).get('cost', 0))
        completion_cost = format_cost(cost_summary.get('completion', {}).get('cost', 0))
        
        cost_info = f"""[bold]Total Cost:[/bold] {total_cost}
[bold]Prompt Cost:[/bold] {prompt_cost}
[bold]Completion Cost:[/bold] {completion_cost}"""
        
        console.print(Panel(cost_info, title="💰 Cost Analysis", border_style="yellow"))
    
    # Annotations
    annotation_names = stats.get('span_annotation_names', [])
    if annotation_names:
        annotations_text = ", ".join(annotation_names[:5])
        if len(annotation_names) > 5:
            annotations_text += f" (+{len(annotation_names)-5} more)"
        
        console.print(Panel(annotations_text, title="🏷️ Available Annotations", border_style="blue"))
    
    # Document evaluations
    doc_eval_names = stats.get('document_evaluation_names', [])
    if doc_eval_names:
        eval_text = ", ".join(doc_eval_names[:5])
        if len(doc_eval_names) > 5:
            eval_text += f" (+{len(doc_eval_names)-5} more)"
        
        console.print(Panel(eval_text, title="📄 Document Evaluations", border_style="magenta"))