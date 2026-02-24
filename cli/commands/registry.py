"""
Registry management commands for the Nasiko CLI.
"""
from datetime import datetime
from typing import Optional

import requests
import typer
from rich.console import Console
from rich.json import JSON
from rich.panel import Panel
from rich.table import Table
import sys
import os
sys.path.append(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from core.settings import APIEndpoints
from core.api_client import get_api_client

console = Console()

def list_agents_command(format_type: str = "table", show_details: bool = False):
    """Get list of all agents from registry"""

    try:
        client = get_api_client()
        response = client.get(APIEndpoints.REGISTRY_ALL_AGENTS, True)
        data = client.handle_response(response, success_message="Agents retrieved successfully")
        
        if not data:
            return
            
        agents = data.get("data", [])
        total = len(agents)

        if not agents:
            console.print("[yellow]No agents found in registry[/yellow]")
            return

        console.print(f"[bold magenta]Agent Registry ({total} agents)[/bold magenta]")

        if format_type == "table":
            display_agents_table(agents, show_details)
        elif format_type == "json":
            display_agents_json(agents)
        elif format_type == "list":
            display_agents_list(agents, show_details)

    except requests.exceptions.ConnectionError:
        console.print(
            "[red]Error: Could not connect to Nasiko API. Make sure the server is running.[/red]"
        )
    except requests.exceptions.Timeout:
        console.print("[red]Error: Request timed out. The server might be busy.[/red]")
    except requests.exceptions.HTTPError as e:
        console.print(f"[red]Error: HTTP {e.response.status_code} - {e.response.text}[/red]")
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")


def get_agent_command(
    agent_identifier: str,
    by_name: bool = True,
    by_agent_id: bool = False,
    format_type: str = "details",
):
    """Get detailed information about a specific agent"""
    client = get_api_client()
    try:
        if by_agent_id:
            response = client.get(APIEndpoints.REGISTRY_BY_AGENT_ID.format(agent_id=agent_identifier), True)
            identifier_type = "Agent ID"
        else:
            response = client.get(APIEndpoints.REGISTRY_BY_AGENT_NAME.format(agent_name=agent_identifier), True)
            identifier_type = "Agent Name"

        if response.status_code == 404:
            console.print(
                f"[red]Agent with {identifier_type} '{agent_identifier}' not found in registry[/red]"
            )
            return

        response.raise_for_status()
        agent_data = response.json()

        if format_type == "json":
            console.print(JSON.from_data(agent_data))
        else:
            display_agent_details(agent_data)

    except requests.exceptions.ConnectionError:
        console.print(
            "[red]Error: Could not connect to Nasiko API. Make sure the server is running.[/red]"
        )
    except requests.exceptions.Timeout:
        console.print("[red]Error: Request timed out. The server might be busy.[/red]")
    except requests.exceptions.HTTPError as e:
        console.print(f"[red]Error: HTTP {e.response.status_code} - {e.response.text}[/red]")
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")


def display_agent_details(agent_data):
    """Display detailed information about an agent"""

    if "data" in agent_data:
        actual_data = agent_data["data"]
    else:
        actual_data = agent_data

    # Basic information panel
    agent_name = actual_data.get("name", "Unknown")
    basic_info = f"""[bold]Name:[/bold] {agent_name}
[bold]ID:[/bold] {actual_data.get('id', 'N/A')}
[bold]Version:[/bold] {actual_data.get('version', 'N/A')}
[bold]Protocol Version:[/bold] {actual_data.get('protocolVersion', 'N/A')}
[bold]Description:[/bold] {actual_data.get('description', 'N/A')}
[bold]URL:[/bold] {actual_data.get('url', 'N/A')}
[bold]Preferred Transport:[/bold] {actual_data.get('preferredTransport', 'N/A')}"""

    console.print(Panel(basic_info, title=f"Agent: {agent_name}", border_style="blue"))

    # Provider information
    provider = actual_data.get("provider", {})
    if provider:
        provider_info = f"""[bold]Organization:[/bold] {provider.get('organization', 'N/A')}
[bold]URL:[/bold] {provider.get('url', 'N/A')}"""
        console.print(Panel(provider_info, title="Provider", border_style="cyan"))

    # URLs and Documentation
    urls_info = ""
    if actual_data.get("iconUrl"):
        urls_info += f"[bold]Icon URL:[/bold] {actual_data['iconUrl']}\n"
    if actual_data.get("documentationUrl"):
        urls_info += f"[bold]Documentation:[/bold] {actual_data['documentationUrl']}\n"

    if urls_info:
        console.print(Panel(urls_info.strip(), title="Resources", border_style="magenta"))

    # Capabilities
    capabilities = actual_data.get("capabilities", {})
    if capabilities and isinstance(capabilities, dict):
        cap_info = ""
        for key, value in capabilities.items():
            cap_info += f"[bold]{key}:[/bold] {value}\n"
        console.print(Panel(cap_info.strip(), title="Capabilities", border_style="green"))

    # Input/Output Modes
    io_info = ""
    default_input_modes = actual_data.get("defaultInputModes", [])
    if default_input_modes:
        io_info += f"[bold]Input Modes:[/bold] {', '.join(default_input_modes)}\n"

    default_output_modes = actual_data.get("defaultOutputModes", [])
    if default_output_modes:
        io_info += f"[bold]Output Modes:[/bold] {', '.join(default_output_modes)}\n"

    if io_info:
        console.print(Panel(io_info.strip(), title="Input/Output Modes", border_style="cyan"))

    # Security
    security_schemes = actual_data.get("securitySchemes", {})
    security = actual_data.get("security", [])
    if security_schemes or security:
        security_info = f"[bold]Security Schemes:[/bold] {len(security_schemes)} configured\n"
        security_info += f"[bold]Security:[/bold] {len(security)} entries"
        console.print(Panel(security_info.strip(), title="Security", border_style="red"))

    # AgentCard Skills
    skills = actual_data.get("skills", [])
    if skills:
        skills_count = len(skills)
        console.print(f"\n[bold yellow]🔧 Agent Skills ({skills_count})[/bold yellow]")

        for i, skill in enumerate(skills, 1):
            skill_info = f"""[bold]ID:[/bold] {skill.get('id', 'N/A')}
[bold]Name:[/bold] {skill.get('name', 'N/A')}
[bold]Description:[/bold] {skill.get('description', 'N/A')}"""

            if skill.get("tags"):
                skill_info += f"\n[bold]Tags:[/bold] {', '.join(skill['tags'])}"

            if skill.get("examples"):
                skill_info += f"\n[bold]Examples:[/bold] {', '.join(skill['examples'])}"

            input_modes = skill.get("inputModes", [])
            if input_modes:
                skill_info += f"\n[bold]Input Modes:[/bold] {', '.join(input_modes)}"

            output_modes = skill.get("outputModes", [])
            if output_modes:
                skill_info += f"\n[bold]Output Modes:[/bold] {', '.join(output_modes)}"

            console.print(
                Panel(
                    skill_info,
                    title=f"Skill {i}: {skill.get('name', 'Unknown')}",
                    border_style="yellow",
                )
            )

    # Additional fields
    additional_info = ""
    if actual_data.get("supportsAuthenticatedExtendedCard") is not None:
        additional_info += f"[bold]Supports Authenticated Extended Card:[/bold] {actual_data['supportsAuthenticatedExtendedCard']}\n"

    signatures = actual_data.get("signatures", [])
    if signatures:
        additional_info += f"[bold]Signatures:[/bold] {len(signatures)} signatures\n"

    additional_interfaces = actual_data.get("additionalInterfaces")
    if additional_interfaces:
        additional_info += (
            f"[bold]Additional Interfaces:[/bold] {len(additional_interfaces)} interfaces\n"
        )

    created_at = actual_data.get("created_at")
    if created_at:
        created_at = datetime.fromisoformat(created_at)
        additional_info += f"[bold]Created at:[/bold] {created_at}\n"

    updated_at = actual_data.get("updated_at")
    if updated_at:
        updated_at = datetime.fromisoformat(updated_at)
        additional_info += f"[bold]Updated at:[/bold] {updated_at}\n"

    if additional_info:
        console.print(
            Panel(additional_info.strip(), title="Additional Information", border_style="cyan")
        )


def display_agent_capabilities(agent_data):
    """Display agent skills from agent card"""

    # Extract from response format
    if "data" in agent_data:
        actual_data = agent_data["data"]
    else:
        actual_data = agent_data

    agent_name = actual_data.get("name", "Unknown")
    skills = actual_data.get("skills", [])

    if not skills or not isinstance(skills, list):
        console.print(f"[yellow]Agent '{agent_name}' has no skills defined[/yellow]")
        return

    console.print(f"[bold magenta]Skills for {agent_name}[/bold magenta]\n")

    # Display each skill with its details
    for skill in skills:
        skill_info = f"[bold]Name:[/bold] {skill.get('name', 'N/A')}\n"
        skill_info += f"[bold]ID:[/bold] {skill.get('id', 'N/A')}\n"
        skill_info += f"[bold]Description:[/bold] {skill.get('description', 'N/A')}\n"

        tags = skill.get("tags", [])
        if tags:
            skill_info += f"[bold]Tags:[/bold] {', '.join(tags)}\n"

        examples = skill.get("examples", [])
        if examples:
            skill_info += f"[bold]Examples:[/bold]\n"
            for example in examples:
                skill_info += f"  • {example}\n"

        console.print(
            Panel(
                skill_info.strip(),
                title=f"Skill: {skill.get('name', 'Unknown')}",
                border_style="green",
            )
        )
        console.print()  # Add spacing between skills


def display_agents_table(agents, show_details=False):
    """Display agents in a table format"""

    table = Table(
        show_header=True,
        header_style="bold magenta",
        row_styles=["none"],
        pad_edge=False,
    )

    # Center aligned columns
    table.add_column("Agent Name", style="blue", width=25, justify="center")
    table.add_column("Agent ID", style="magenta", width=50, justify="center")
    table.add_column("Tags", style="green", width=20, justify="center")

    if show_details:
        table.add_column("Description", style="white", max_width=50, justify="center")

    for agent in agents:
        agent_name = agent.get("name", "Unknown")
        agent_id = agent.get("agent_id", "N/A")
        agent_tags = agent.get("tags", [])
        agent_description = agent.get("description", "No description")

        row = [agent_name, agent_id, ", ".join(agent_tags)]

        if show_details:
            row.append(agent_description)

        # Add the actual row
        table.add_row(*row, end_section=True)

    console.print(table)

def display_agents_json(agents):
    """Display agents in JSON format"""
    console.print(JSON.from_data(agents))


def display_agents_list(agents, show_details=False):
    """Display agents in a list format"""

    for i, agent in enumerate(agents, 1):
        agent_name = agent.get("name", "Unknown")
        agent_id = agent.get("id", "N/A")
        agent_version = agent.get("version", "N/A")
        agent_description = agent.get("description", "No description")

        skills_count = len(agent.get("skills", []))

        agent_info = f"[bold blue]{i}. {agent_name}[/bold blue]\n"
        agent_info += f"   Agent ID: {agent_id}\n"
        agent_info += f"   URL: {agent.get('url', 'N/A')}\n"
        agent_info += f"   Version: {agent_version}\n"
        agent_info += f"   Skills: {skills_count} skills"

        if show_details:
            agent_info += f"\n   Description: {agent_description}"

        console.print(agent_info)
        console.print()  # Add spacing between agents


def api_docs_command():
    """Get API documentation and Swagger link"""

    try:
        # Check if the API server is running and get the docs URL
        client = get_api_client()
        base_url = client.base_url
        
        docs_url = f"{base_url}/docs"
        redoc_url = f"{base_url}/redoc"
        openapi_url = f"{base_url}/openapi.json"

        # Test if the server is running
        response = client.get(APIEndpoints.HEALTHCHECK, require_auth=False, timeout=5)
        response.raise_for_status()

        docs_info = f"""[bold]Nasiko API Documentation[/bold]

[bold cyan]📚 Interactive API Documentation:[/bold cyan]
• Swagger UI: {docs_url}
• ReDoc: {redoc_url}

[bold cyan]📄 OpenAPI Specification:[/bold cyan]
• JSON Format: {openapi_url}

[bold cyan]🔗 Key API Endpoints:[/bold cyan]
• Agent Registry: /api/v1/registries (GET, POST)
• Agent Details: /api/v1/agent/name/{{name}} | /api/v1/agent/id/{{id}}
• Traces: /api/v1/traces (POST)
• Agent Upload: /api/v1/agents/upload (POST)
• Upload Status: /api/v1/upload-status/{{user_id}} (GET)
• Chat Sessions: /api/v1/chat/sessions (GET, POST, DELETE)
• Chat History: /api/v1/chat/history/{{session_id}} (GET)
• GitHub Auth: /api/v1/auth/github/login (GET)
• N8N Integration: /api/v1/n8n/credentials (GET, POST, PUT, DELETE)
• Search: /api/v1/search/agents (POST)

[bold cyan]💡 Usage:[/bold cyan]
Visit the Swagger UI link above for interactive documentation where you can:
• Explore all available endpoints with full schemas
• Test API calls directly from your browser
• View detailed request/response examples
• Download the complete OpenAPI specification"""

        console.print(Panel(docs_info, title="API Documentation", border_style="blue"))

        console.print(f"\n[green]✅ API server is running and accessible[/green]")
        console.print(
            f"[yellow]💡 Open {docs_url} in your browser for interactive documentation[/yellow]"
        )

    except requests.exceptions.ConnectionError:
        console.print(
            "[red]Error: Could not connect to Nasiko API. Make sure the server is running.[/red]"
        )
        client = get_api_client()
        console.print(f"[yellow]Expected server URL: {client.base_url}[/yellow]")
    except requests.exceptions.Timeout:
        console.print("[red]Error: Request timed out. The server might be starting up.[/red]")
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")

        # Still show the docs info even if server is down
        client = get_api_client()
        base_url = client.base_url
        docs_url = f"{base_url}/docs"

        console.print(f"\n[yellow]📚 When the server is running, visit: {docs_url}[/yellow]")
