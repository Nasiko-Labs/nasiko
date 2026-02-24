"""
Agent upload commands for Nasiko CLI.
"""

import tempfile
import zipfile
from pathlib import Path
from typing import Optional
import typer
from rich.console import Console

from core.settings import APIEndpoints
from core.api_client import get_api_client

console = Console()

def upload_zip_command(zip_file: str, agent_name: Optional[str] = None):
    """
    Upload and deploy an agent from a .zip file.
    """
    print("--- agent zip upload ---")
    
    # Validate zip file exists
    zip_path = Path(zip_file).resolve()
    if not zip_path.exists():
        print(f"error: zip file does not exist: {zip_file}")
        raise typer.Exit(1)
    
    if not zip_path.is_file():
        print(f"error: path is not a file: {zip_file}")
        raise typer.Exit(1)
    
    if not zip_path.name.lower().endswith('.zip'):
        print(f"error: file must be a .zip file: {zip_file}")
        raise typer.Exit(1)
    
    print(f"uploading agent from: {zip_path}")
    if agent_name:
        print(f"agent name: {agent_name}")
    else:
        print("agent name: auto-detect from zip contents")
    
    try:
        client = get_api_client()
        additional_data = {}
        if agent_name:
            additional_data['agent_name'] = agent_name

        response = client.upload_file(
            endpoint=APIEndpoints.AGENT_UPLOAD,
            file_path=str(zip_path),
            additional_data=additional_data
        )
        
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)
            
        # Handle new nested response format
        data = result.get('data', result)
        
        if data.get('success'):
            print(f"status: {data['status']}")
            print(f"\n✓ successfully uploaded agent: '{data['agent_name']}'")

            if data.get('agentcard_generated'):
                print("✓ AgentCard.json generated automatically")
            elif data.get('capabilities_generated'):
                print("✓ capabilities.json generated automatically")

            if data.get('orchestration_triggered'):
                print("✓ agent orchestration triggered")
            else:
                print("⚠ warning: agent orchestration failed to trigger")
                
        else:
            print(f"\n✗ upload failed: {data.get('status', 'unknown error')}")
            if data.get('validation_errors'):
                print("validation errors:")
                for error in data['validation_errors']:
                    print(f"  - {error}")
            raise typer.Exit(1)
                
    except typer.Exit:
        raise
    except Exception as e:
        print(f"\nerror: unexpected error during upload: {e}")
        raise typer.Exit(1)


def upload_directory_command(directory_path: str, agent_name: Optional[str] = None):
    """
    Upload and deploy an agent from a local directory by creating a temporary zip file.
    """
    print("--- agent directory upload ---")
    
    # Validate directory exists
    dir_path = Path(directory_path).resolve()
    if not dir_path.exists():
        print(f"error: directory does not exist: {directory_path}")
        raise typer.Exit(1)
    
    if not dir_path.is_dir():
        print(f"error: path is not a directory: {directory_path}")
        raise typer.Exit(1)
    
    print(f"uploading agent from: {dir_path}")
    if agent_name:
        print(f"agent name: {agent_name}")
    else:
        print("agent name: auto-detect from directory")
    
    # Create a temporary zip file from the directory
    try:
        with tempfile.NamedTemporaryFile(suffix='.zip', delete=False) as temp_zip:
            temp_zip_path = temp_zip.name
        
        print(f"creating temporary zip file...")
        with zipfile.ZipFile(temp_zip_path, 'w', zipfile.ZIP_DEFLATED) as zipf:
            for file_path in dir_path.rglob('*'):
                if file_path.is_file():
                    # Calculate relative path within the directory
                    arcname = file_path.relative_to(dir_path)
                    zipf.write(file_path, arcname)
        
        # Upload the temporary zip file using the API client
        print(f"uploading zip file...")
        client = get_api_client()
        additional_data = {}
        if agent_name:
            additional_data['agent_name'] = agent_name

        response = client.upload_file(
            endpoint=APIEndpoints.AGENT_UPLOAD,
            file_path=temp_zip_path,
            additional_data=additional_data
        )
        
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)
            
        # Handle new nested response format
        data = result.get('data', result)
        
        if data.get('success'):
            print(f"status: {data['status']}")
            print(f"\n✓ successfully uploaded agent: '{data['agent_name']}'")

            if data.get('agentcard_generated'):
                print("✓ AgentCard.json generated automatically")
            elif data.get('capabilities_generated'):
                print("✓ capabilities.json generated automatically")

            if data.get('orchestration_triggered'):
                print("✓ agent orchestration triggered")
            else:
                print("⚠ warning: agent orchestration failed to trigger")
                
        else:
            print(f"\n✗ upload failed: {data.get('status', 'unknown error')}")
            if data.get('validation_errors'):
                print("validation errors:")
                for error in data['validation_errors']:
                    print(f"  - {error}")
            raise typer.Exit(1)
                
    except typer.Exit:
        raise
    except Exception as e:
        print(f"\nerror: unexpected error during upload: {e}")
        raise typer.Exit(1)
    finally:
        # Clean up temporary zip file
        try:
            Path(temp_zip_path).unlink()
        except (OSError, NameError):
            # Ignore if file doesn't exist or temp_zip_path was never created
            pass


def list_user_uploaded_agents_command():
    """
    List uploaded agents for the current user.
    """
    console.print("[bold magenta]--- My Uploaded Agents ---[/bold magenta]")

    try:
        client = get_api_client()

        console.print("[yellow]🔄 Fetching uploaded agents...[/yellow]")

        response = client.get(APIEndpoints.USER_UPLOAD_AGENTS)
        result = client.handle_response(response)

        if result is None:
            raise typer.Exit(1)

        agents = result.get("data", [])

        if not agents or len(agents) == 0:
            console.print("[yellow]No uploaded agents found.[/yellow]")
            return

        console.print(f"\n[bold magenta]Found {len(agents)} uploaded agents:[/bold magenta]")

        for i, agent in enumerate(agents, 1):
            agent_id = agent.get("agent_id", "N/A")
            agent_name = agent.get("agent_name", "N/A")
            description = agent.get("description", "No description")
            url = agent.get("url")
            tags = agent.get("tags", [])
            skills = agent.get("skills", [])

            upload_info = agent.get("upload_info", {})
            upload_type = upload_info.get("upload_type", "unknown")
            upload_status = upload_info.get("upload_status", "unknown")

            # Status color coding
            if upload_status == "Active":
                status_color = "green"
                status_icon = "✅"
            elif upload_status == "Setting Up":
                status_color = "yellow"
                status_icon = "⏳"
            elif upload_status == "Failed":
                status_color = "red"
                status_icon = "❌"
            else:
                status_color = "blue"
                status_icon = "ℹ️"

            console.print(f"\n  [cyan]{i:2}.[/cyan] [green]{agent_name}[/green]")
            if agent_id and agent_id != "":
                console.print(f"      [dim]ID: {agent_id}[/dim]")
            else:
                console.print(f"      [dim]ID: Not assigned yet[/dim]")

            console.print(
                f"      [{status_color}]{status_icon} {upload_status}[/{status_color}] • [blue]{upload_type.replace('_', ' ').title()}[/blue]")

            if tags:
                tags_str = ", ".join(tags)
                console.print(f"      [yellow]Tags: {tags_str}[/yellow]")

            if skills:
                skills_count = len(skills)
                console.print(f"      [magenta]Skills: {skills_count}[/magenta]")

            if url:
                console.print(f"      [cyan]URL: {url}[/cyan]")

            console.print(f"      [dim]{description}[/dim]")

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: unexpected error fetching uploaded agents: {e}[/red]")
        raise typer.Exit(1)