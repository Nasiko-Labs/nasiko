"""
Chat send command for the Nasiko CLI.
"""

import typer
import requests
import uuid
import json
from typing import Optional
from rich.console import Console
from rich.panel import Panel
from rich.progress import Progress, SpinnerColumn, TextColumn
from rich.json import JSON
from auth.auth_manager import get_auth_manager

console = Console()


def send_message_command(url: str, message: str, session_id: str):
    """Send a message to an agent using JSON-RPC format and display the response."""

    try:
        # Get authentication headers
        auth_manager = get_auth_manager()
        if not auth_manager.is_logged_in():
            console.print("[red]Error: Please login first using 'nasiko login'[/red]")
            raise typer.Exit(1)

        if not auth_manager.refresh_token_if_needed():
            console.print("[red]Error: Authentication failed. Please login again.[/red]")
            raise typer.Exit(1)

        # Use session_id as the request ID
        request_id = session_id
        
        # Generate a unique context ID
        context_id = str(uuid.uuid4())
        
        # Generate a unique message ID
        message_id = str(uuid.uuid4())
        
        # Prepare JSON-RPC request payload based on the provided format
        payload = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "message/send",
            "params": {
                "message": {
                    "role": "user",
                    "parts": [
                        {
                            "kind": "text",
                            "text": message
                        }
                    ],
                    "messageId": message_id
                }
            }
        }
        
        # Display request info
        console.print(f"[bold cyan]Sending message to:[/bold cyan] {url}")
        console.print(f"[bold cyan]Session ID:[/bold cyan] {session_id}")
        console.print(f"[bold cyan]Message:[/bold cyan] {message}")
        console.print()
        
        # Send request with loading indicator
        with Progress(
            SpinnerColumn(),
            TextColumn("[progress.description]{task.description}"),
            console=console
        ) as progress:
            task = progress.add_task("Sending message and waiting for response...", total=None)
            
            headers = {
                "Content-Type": "application/json"
            }

            # Add authentication headers
            auth_headers = auth_manager.get_auth_headers()
            if auth_headers:
                headers.update(auth_headers)

            response = requests.post(url, json=payload, headers=headers, timeout=60)
            progress.remove_task(task)
        
        # Handle response
        if response.status_code == 200:
            try:
                response_data = response.json()
                
                # Parse and display the formatted response
                display_agent_response(response_data)
                
            except json.JSONDecodeError:
                console.print("[red]Error: Invalid JSON response from agent[/red]")
                console.print(f"[red]Response content:[/red] {response.text}")
        else:
            console.print(f"[red]Error: HTTP {response.status_code}[/red]")
            console.print(f"[red]Response:[/red] {response.text}")
            
    except requests.exceptions.ConnectionError:
        console.print(f"[red]Error: Could not connect to agent at {url}[/red]")
        console.print("[red]Make sure the agent is running and the URL is correct.[/red]")
    except requests.exceptions.Timeout:
        console.print("[red]Error: Request timed out. The agent might be taking too long to respond.[/red]")
    except Exception as e:
        console.print(f"[red]Error: {str(e)}[/red]")


def display_agent_response(response_data):
    """Parse and display the agent response - show text from artifacts as the main response."""
    
    try:
        # Extract the result from JSON-RPC response
        result = response_data.get('result', {})
        
        if not result:
            console.print("[yellow]No result found in response[/yellow]")
            return
        
        # Look for text content in artifacts first (main response)
        artifacts = result.get('artifacts', [])
        response_found = False
        
        if artifacts:
            for artifact in artifacts:
                artifact_parts = artifact.get('parts', [])
                
                for part in artifact_parts:
                    if part.get('kind') == 'text':
                        text = part.get('text', '')
                        if text:
                            console.print("[bold green]Agent Response:[/bold green]")
                            console.print(Panel(
                                text, 
                                title="💬 Agent Reply", 
                                border_style="green",
                                padding=(1, 1)
                            ))
                            response_found = True
        
        # If no artifacts, check message parts
        if not response_found:
            message = result.get('message', {})
            parts = message.get('parts', [])
            
            for part in parts:
                if part.get('kind') == 'text':
                    text_content = part.get('text', '')
                    if text_content:
                        console.print("[bold green]Agent Response:[/bold green]")
                        console.print(Panel(
                            text_content, 
                            title="💬 Agent Reply", 
                            border_style="green",
                            padding=(1, 1)
                        ))
                        response_found = True
        
        if not response_found:
            console.print("[yellow]No text response found in the agent's reply[/yellow]")
        
    except Exception as e:
        console.print(f"[red]Error parsing response: {str(e)}[/red]")
        console.print("[yellow]Displaying raw response:[/yellow]")
        console.print(JSON.from_data(response_data))