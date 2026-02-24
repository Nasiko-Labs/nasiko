"""
N8N Integration commands for Nasiko CLI.
"""

import typer
from typing import Optional
from rich.console import Console
from rich.panel import Panel

from core.api_client import get_api_client
from core.settings import APIEndpoints

console = Console()


def register_workflow(
    workflow_id: str,
    agent_name: Optional[str] = None,
    agent_description: Optional[str] = None
):
    """Register an N8N workflow as an agent."""

    try:
        console.print(f"[yellow]🔄 Registering N8N workflow {workflow_id} as an agent...[/yellow]")
        console.print("[dim]This may take a few moments to complete.[/dim]")
        
        client = get_api_client()
        payload = {"workflow_id": workflow_id}
        if agent_name:
            payload["agent_name"] = agent_name
        if agent_description:
            payload["agent_description"] = agent_description

        response = client.post(APIEndpoints.N8N_REGISTER, payload)
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)

        if result.get('success'):
            console.print(f"[green]✅ Successfully registered N8N workflow as agent[/green]")
            if result.get('agent_name'):
                console.print(f"[cyan]Agent Name: {result['agent_name']}[/cyan]")
            if result.get('agent_id'):
                console.print(f"[cyan]Agent ID: {result['agent_id']}[/cyan]")
            if result.get('webhook_url'):
                console.print(f"[cyan]Webhook URL: {result['webhook_url']}[/cyan]")
            if result.get('upload_id'):
                console.print(f"[cyan]Upload ID: {result['upload_id']}[/cyan]")
        else:
            console.print(f"[red]Registration failed: {result.get('message', 'Unknown error')}[/red]")
            raise typer.Exit(1)

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")
        raise typer.Exit(1)


def connect_n8n(
    connection_name: str,
    n8n_url: str,
    api_key: str
):
    """Save and test N8N credentials."""
    try:
        client = get_api_client()
        payload = {
            "connection_name": connection_name,
            "n8n_url": n8n_url,
            "api_key": api_key
        }
        response = client.post(APIEndpoints.N8N_CONNECT, payload)
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)
            
        data = result.get('data', result)

        if data.get('connection_status') == 'success':
            console.print(f"[green]✅ Successfully connected to N8N instance[/green]")
            console.print(f"[cyan]Connection: {connection_name}[/cyan]")
            console.print(f"[cyan]Status: {data.get('connection_status', 'active')}[/cyan]")
        else:
            console.print(f"[red]Connection failed: {result.get('message', 'Unknown error')}[/red]")
            raise typer.Exit(1)

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")
        raise typer.Exit(1)


def get_n8n_credentials():
    """Get saved N8N credentials."""

    try:
        client = get_api_client()
        response = client.get(APIEndpoints.N8N_CREDENTIALS)
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)
            
        # Response format: {success: bool, message: str, data: UserN8NCredentialResponse}
        data = result.get('data')

        if result.get('success') and data:
            status = "Active" if data.get('is_active') else "Inactive"
            cred_info = f"""[bold]Connection Name:[/bold] {data.get('connection_name', 'N/A')}
[bold]N8N URL:[/bold] {data.get('n8n_url', 'N/A')}
[bold]Status:[/bold] {status}"""

            if data.get('last_tested'):
                cred_info += f"\n[bold]Last Tested:[/bold] {data['last_tested']}"
            if data.get('created_at'):
                cred_info += f"\n[bold]Created:[/bold] {data['created_at']}"
            if data.get('updated_at'):
                cred_info += f"\n[bold]Updated:[/bold] {data['updated_at']}"

            console.print(Panel(cred_info, title="N8N Credentials", border_style="cyan"))
        else:
            console.print(f"[yellow]{result.get('message', 'No N8N credentials found')}[/yellow]")

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")
        raise typer.Exit(1)


def update_n8n_credentials(
    connection_name: Optional[str] = None,
    n8n_url: Optional[str] = None,
    api_key: Optional[str] = None,
    is_active: Optional[bool] = None
):
    """Update N8N credentials."""

    if not any([connection_name, n8n_url, api_key, is_active is not None]):
        console.print("[red]Error: At least one field must be provided to update[/red]")
        raise typer.Exit(1)

    try:
        client = get_api_client()
        payload = {}
        if connection_name:
            payload["connection_name"] = connection_name
        if n8n_url:
            payload["n8n_url"] = n8n_url
        if api_key:
            payload["api_key"] = api_key
        if is_active is not None:
            payload["is_active"] = is_active

        response = client.put(APIEndpoints.N8N_CREDENTIALS, payload)
        result = client.handle_response(response, success_message="Successfully updated N8N credentials")
        if result is None:
            raise typer.Exit(1)

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")
        raise typer.Exit(1)


def delete_n8n_credentials():
    """Delete N8N credentials permanently."""

    console.print("[yellow]Are you sure you want to delete N8N credentials?[/yellow]")
    console.print("[yellow]This action cannot be undone.[/yellow]")

    confirm = typer.confirm("Continue with deletion?")
    if not confirm:
        console.print("[blue]Deletion cancelled[/blue]")
        return

    try:
        client = get_api_client()
        response = client.delete(APIEndpoints.N8N_CREDENTIALS)
        result = client.handle_response(response, success_message="Successfully deleted N8N credentials")
        if result is None:
            raise typer.Exit(1)

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")
        raise typer.Exit(1)


def list_n8n_workflows(
    active_only: bool = True,
    limit: int = 100
):
    """List N8N workflows from connected instance."""

    try:
        client = get_api_client()
        params = {
            "active_only": str(active_only).lower(),
            "limit": limit
        }

        response = client.get(APIEndpoints.N8N_WORKFLOWS, params=params)
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)
            
        # Response format: {workflows: List[WorkflowSummary], total_count: int, connection_name: str, message: str}
        workflows = result.get('workflows', [])
        total_count = result.get('total_count', len(workflows))

        if workflows:
            console.print(f"[bold magenta]N8N Workflows ({total_count} total, {len(workflows)} shown)[/bold magenta]\n")

            for wf in workflows:
                wf_info = f"[bold cyan]• {wf.get('name', 'N/A')}[/bold cyan] (ID: {wf.get('id', 'N/A')})\n"
                wf_info += f"  Active: {wf.get('active', False)}"

                if wf.get('is_chat_workflow'):
                    wf_info += " | Chat Workflow: Yes"
                if wf.get('nodes_count'):
                    wf_info += f" | Nodes: {wf['nodes_count']}"
                if wf.get('last_updated'):
                    wf_info += f" | Updated: {wf['last_updated']}"
                if wf.get('tags'):
                    wf_info += f" | Tags: {', '.join(wf['tags'])}"

                console.print(wf_info)
        else:
            console.print(f"[yellow]{result.get('message', 'No workflows found')}[/yellow]")

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")
        raise typer.Exit(1)
