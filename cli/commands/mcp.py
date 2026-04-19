"""
MCP management commands for the Nasiko CLI.
"""

from typing import List
import requests

from rich.console import Console
from rich.json import JSON
from rich.table import Table

from core.settings import APIEndpoints
from core.api_client import get_api_client

console = Console()


def list_mcp_servers_command(format_type: str = "table"):
    """List all published MCP servers."""
    try:
        client = get_api_client()
        response = client.get(APIEndpoints.MCP_SERVERS, True)
        data = client.handle_response(
            response, success_message="MCP servers retrieved successfully"
        )

        if not data:
            return

        servers = data.get("data", [])
        if not servers:
            console.print("[yellow]No MCP servers found.[/yellow]")
            return

        if format_type == "json":
            console.print(JSON.from_data(servers))
            return

        table = Table(title=f"Published MCP Servers ({len(servers)})")
        table.add_column("Server ID", style="cyan", no_wrap=True)
        table.add_column("Name", style="magenta")
        table.add_column("Version", style="green")
        table.add_column("Tools", justify="right")
        table.add_column("Resources", justify="right")
        table.add_column("Prompts", justify="right")
        table.add_column("Bridge URL", style="yellow")

        for server in servers:
            table.add_row(
                str(server.get("server_id", "")),
                str(server.get("name") or "-"),
                str(server.get("version") or "-"),
                str(server.get("tools", 0)),
                str(server.get("resources", 0)),
                str(server.get("prompts", 0)),
                str(server.get("bridge_url") or "-"),
            )

        console.print(table)

    except requests.exceptions.ConnectionError:
        console.print(
            "[red]Error: Could not connect to Nasiko API. Make sure the server is running.[/red]"
        )
    except requests.exceptions.Timeout:
        console.print("[red]Error: Request timed out. The server might be busy.[/red]")
    except requests.exceptions.HTTPError as e:
        console.print(
            f"[red]Error: HTTP {e.response.status_code} - {e.response.text}[/red]"
        )
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")


def get_mcp_manifest_command(server_id: str, format_type: str = "pretty"):
    """Get generated MCP manifest for one server."""
    try:
        client = get_api_client()
        response = client.get(APIEndpoints.MCP_MANIFEST.format(server_id=server_id), True)
        data = client.handle_response(
            response, success_message=f"MCP manifest retrieved for {server_id}"
        )

        if not data:
            return

        manifest = data.get("data", {})

        if format_type == "json":
            console.print(JSON.from_data(manifest))
            return

        tools = manifest.get("tools", [])
        resources = manifest.get("resources", [])
        prompts = manifest.get("prompts", [])

        console.print(f"[bold magenta]MCP Manifest: {server_id}[/bold magenta]")
        console.print(f"Name: {manifest.get('name', '-')}")
        console.print(f"Version: {manifest.get('version', '-')}")
        console.print(f"Tools: {len(tools)}")
        console.print(f"Resources: {len(resources)}")
        console.print(f"Prompts: {len(prompts)}")

        if tools:
            tool_table = Table(title="Tools")
            tool_table.add_column("Name", style="cyan")
            tool_table.add_column("Description", style="white")
            for tool in tools:
                tool_table.add_row(
                    str(tool.get("name", "")),
                    str(tool.get("description") or "-"),
                )
            console.print(tool_table)

        if resources:
            resource_table = Table(title="Resources")
            resource_table.add_column("Name", style="cyan")
            resource_table.add_column("Description", style="white")
            for resource in resources:
                resource_table.add_row(
                    str(resource.get("name", "")),
                    str(resource.get("description") or "-"),
                )
            console.print(resource_table)

        if prompts:
            prompt_table = Table(title="Prompts")
            prompt_table.add_column("Name", style="cyan")
            prompt_table.add_column("Description", style="white")
            for prompt in prompts:
                prompt_table.add_row(
                    str(prompt.get("name", "")),
                    str(prompt.get("description") or "-"),
                )
            console.print(prompt_table)

    except requests.exceptions.ConnectionError:
        console.print(
            "[red]Error: Could not connect to Nasiko API. Make sure the server is running.[/red]"
        )
    except requests.exceptions.Timeout:
        console.print("[red]Error: Request timed out. The server might be busy.[/red]")
    except requests.exceptions.HTTPError as e:
        console.print(
            f"[red]Error: HTTP {e.response.status_code} - {e.response.text}[/red]"
        )
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")


def get_mcp_associations_command(agent_id: str):
    """Get MCP associations configured for an agent."""
    try:
        client = get_api_client()
        response = client.get(
            APIEndpoints.MCP_AGENT_ASSOCIATIONS.format(agent_id=agent_id),
            True,
        )
        data = client.handle_response(
            response,
            success_message=f"MCP associations retrieved for agent {agent_id}",
        )

        if not data:
            return

        payload = data.get("data", {})
        associated = payload.get("associated_mcp_servers", [])
        bridge_urls = payload.get("mcp_bridge_urls", {})

        console.print(f"[bold magenta]MCP Associations for {agent_id}[/bold magenta]")
        if not associated:
            console.print("[yellow]No MCP associations configured.[/yellow]")
            return

        table = Table()
        table.add_column("MCP Server", style="cyan")
        table.add_column("Bridge URL", style="yellow")

        for server_id in associated:
            table.add_row(server_id, bridge_urls.get(server_id, "-"))

        console.print(table)

    except requests.exceptions.ConnectionError:
        console.print(
            "[red]Error: Could not connect to Nasiko API. Make sure the server is running.[/red]"
        )
    except requests.exceptions.Timeout:
        console.print("[red]Error: Request timed out. The server might be busy.[/red]")
    except requests.exceptions.HTTPError as e:
        console.print(
            f"[red]Error: HTTP {e.response.status_code} - {e.response.text}[/red]"
        )
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")


def set_mcp_associations_command(agent_id: str, server_ids: List[str], replace: bool):
    """Set or merge MCP associations for an agent."""
    try:
        client = get_api_client()
        payload = {
            "mcp_server_ids": server_ids,
            "replace": replace,
        }

        response = client.put(
            APIEndpoints.MCP_AGENT_ASSOCIATIONS.format(agent_id=agent_id),
            data=payload,
            require_auth=True,
        )

        data = client.handle_response(
            response,
            success_message=f"Updated MCP associations for agent {agent_id}",
        )

        if not data:
            return

        result = data.get("data", {})
        associated = result.get("associated_mcp_servers", [])
        console.print(
            f"[green]Agent {agent_id} now associated with: {', '.join(associated) if associated else 'no servers'}[/green]"
        )

    except requests.exceptions.ConnectionError:
        console.print(
            "[red]Error: Could not connect to Nasiko API. Make sure the server is running.[/red]"
        )
    except requests.exceptions.Timeout:
        console.print("[red]Error: Request timed out. The server might be busy.[/red]")
    except requests.exceptions.HTTPError as e:
        console.print(
            f"[red]Error: HTTP {e.response.status_code} - {e.response.text}[/red]"
        )
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")
