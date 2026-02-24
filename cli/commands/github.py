"""
GitHub authentication and repository commands for Nasiko CLI.
"""

import time
import webbrowser
from typing import Optional, Any, Union

import typer
from rich.console import Console

from core.settings import APIEndpoints
from core.api_client import get_api_client

console = Console()


def get_github_status():
    """Fetches the GitHub access token from the backend."""
    try:
        client = get_api_client()
        response = client.get(APIEndpoints.GITHUB_TOKEN)
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)

        if result.get('success'):
            console.print(f"[green]GitHub is connected (username: {result.get('username')})[/]")
        else:
            console.print(f"[red]GitHub is not connected[/]")
        return result
    except Exception as e:
        console.print(f"[red]Error: could not fetch token from the backend: {e}[/red]")
        raise typer.Exit(1)


def login_command():
    """
    Authenticate with GitHub via the Nasiko backend automatically.
    """
    console.print("[bold magenta]--- GitHub Authentication ---[/bold magenta]")
    console.print("A browser window will open for you to authorize the application.")

    try:
        client = get_api_client()
        
        # The login endpoint now handles authentication automatically and returns a redirect
        console.print("\n[cyan]Initiating GitHub login...[/cyan]")
        
        # Call the login endpoint which will return an auth URL
        response = client.get(APIEndpoints.GITHUB_LOGIN)
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)
        
        # Extract auth URL from JSON response
        auth_url = result.get("auth_url")
        if not auth_url:
            console.print("[red]No auth URL found in response[/red]")
            raise typer.Exit(1)
            
        console.print(f"[cyan]Opening authentication URL in your browser:[/cyan] {auth_url}")
        
        try:
            webbrowser.open(auth_url)
        except Exception:
            console.print("[yellow]Could not automatically open browser. Please copy and paste this link manually:[/yellow]")
            console.print(f"[cyan]{auth_url}[/cyan]")
            
    except Exception as e:
        console.print(f"[red]Error: could not initiate GitHub login: {e}[/red]")
        console.print("[yellow]You can try accessing the login URL manually in your browser:[/yellow]")
        console.print(f"[cyan]{APIEndpoints.GITHUB_LOGIN}[/cyan]")
        raise typer.Exit(1)

    # Wait a moment for the browser to hit the login endpoint and initialize the session
    time.sleep(2)

    console.print("\n[yellow]Please complete the authorization in your browser...[/yellow]")
    console.print("[yellow]Waiting for you to complete authentication...[/yellow]")

    # Poll for token with timeout
    timeout_seconds = 180  # 3 minutes
    poll_interval_seconds = 3
    start_time = time.time()
    token_found = False

    while time.time() - start_time < timeout_seconds:
        try:
            # Check if token is available
            response = client.get(APIEndpoints.GITHUB_TOKEN)
            
            # If we get a successful response, check if we have a valid token
            if response.status_code == 200:
                result = response.json()
                # Check if GitHub is connected and token is valid
                if result.get("success") and result.get("status") == "connected":
                    username = result.get("username", "")
                    console.print(f"\n[green]✅ Successfully authenticated with GitHub as {username}![/green]")
                    token_found = True
                    break
            
            # Token not ready yet, continue polling
            console.print(".", end="")
            time.sleep(poll_interval_seconds)
            
        except Exception:
            # Token endpoint might return error while auth is pending, continue polling
            console.print(".", end="")
            time.sleep(poll_interval_seconds)

    if not token_found:
        console.print(f"\n[red]Error: Login timed out after {timeout_seconds} seconds. Please try again.[/red]")
        console.print("[yellow]You can manually check your authentication status with 'nasiko github-token'[/yellow]")
        raise typer.Exit(1)


def logout_command():
    """
    Logout from GitHub by clearing the access token from the backend.
    """
    console.print("[bold magenta]--- GitHub Logout ---[/bold magenta]")

    try:
        client = get_api_client()
        response = client.post(APIEndpoints.GITHUB_LOGOUT, data=None)
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)
            
        if result.get("success"):
            console.print("[green]✅ Successfully logged out from GitHub.[/green]")
            console.print("All GitHub authentication sessions have been cleared.")
        else:
            console.print(f"[red]Error: logout failed: {result.get('message', 'unknown error')}[/red]")
            raise typer.Exit(1)

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"[red]Error: unexpected error during logout: {e}[/red]")
        raise typer.Exit(1)


def list_repos_command():
    """
    List your accessible GitHub repositories.
    """
    console.print("[cyan]Fetching your repositories...[/cyan]")

    try:
        client = get_api_client()
        response = client.get(APIEndpoints.GITHUB_REPOSITORY)
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)
            
        repositories = result.get("repositories", [])
        total = result.get("total", 0)
        
        if not repositories:
            console.print("[yellow]No repositories available.[/yellow]")
            return None
            
        console.print(f"\n[bold magenta]Found {total} repositories:[/bold magenta]")
        
        for i, repo in enumerate(repositories, 1):
            repo_name = repo["full_name"]
            description = repo.get("description") or "No description"
            is_private = "🔒 Private" if repo["private"] else "🌐 Public"
            default_branch = repo.get("default_branch", "main")
            
            console.print(f"  [cyan]{i:2}.[/cyan] [green]{repo_name}[/green]")
            console.print(f"      [dim]{description}[/dim]")
            console.print(f"      [yellow]{is_private}[/yellow] • [blue]Branch: {default_branch}[/blue]")
            console.print()
        
        return result
    except Exception as e:
        console.print(f"[red]Error: could not fetch repositories: {e}[/red]")
        raise typer.Exit(1)


def clone_command(repo: Optional[str], branch: Optional[str]):
    """
    Clone a GitHub repository and upload it as an agent using the server-side clone API.
    If repo is provided, clone that repo directly.
    If repo is None, show a list of repositories to select from.
    """
    if repo:
        # Direct clone mode - repo specified
        repo_full_name = _parse_repo_argument(repo)
        console.print(f"[bold magenta]--- Cloning and uploading repository: {repo_full_name} ---[/bold magenta]")
    else:
        # Interactive mode - select from list
        repo_full_name = _select_repo_from_list()
        if not repo_full_name:
            return
        console.print(f"[bold magenta]--- Cloning and uploading selected repository: {repo_full_name} ---[/bold magenta]")

    # Default to main branch if not specified
    if not branch:
        branch = "main"

    console.print(f"[cyan]Repository: {repo_full_name}[/cyan]")
    console.print(f"[cyan]Branch: {branch}[/cyan]")

    try:
        client = get_api_client()
        
        # Use the server-side GitHub clone API
        console.print("[yellow]🔄 Initiating server-side repository clone and agent upload...[/yellow]")
        console.print("[dim]This may take a few moments to complete.[/dim]")
        
        clone_request = {
            "repository_full_name": repo_full_name,
            "branch": branch,
            "agent_name": None  # Let the server auto-detect
        }
        
        response = client.post(APIEndpoints.GITHUB_CLONE, clone_request)
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)

        # Handle response format (should match AgentUploadResponse)
        data = result.get("data", result)

        if data.get("success"):
            console.print(f"[cyan]Status: {data['status']}[/cyan]")
            console.print(f"\n[green]✅ Successfully cloned and uploaded agent: '{data['agent_name']}' [/green]")

            if data.get("capabilities_generated"):
                console.print("[green]✅ capabilities.json generated automatically[/green]")

            if data.get("orchestration_triggered"):
                console.print("[green]✅ Agent orchestration triggered[/green]")
            else:
                console.print("[yellow]⚠ Warning: agent orchestration failed to trigger[/yellow]")

        else:
            console.print(f"\n[red]✗ Clone and upload failed: {data.get('status', 'unknown error')}[/red]")
            if data.get("validation_errors"):
                console.print("[red]Validation errors:[/red]")
                for error in data["validation_errors"]:
                    console.print(f"[red]  - {error}[/red]")
            raise typer.Exit(1)

    except typer.Exit:
        raise
    except Exception as e:
        console.print(f"\n[red]Error: unexpected error during clone and upload: {e}[/red]")
        raise typer.Exit(1)


def _parse_repo_argument(repo: str) -> str:
    """
    Parse repository argument and return full repository name (owner/repo).
    Handles both full URLs and owner/repo format.
    """
    if repo.startswith("https://github.com/"):
        # Extract owner/repo from URL
        repo_part = repo.replace("https://github.com/", "").rstrip("/")
        # Remove .git suffix if present
        if repo_part.endswith(".git"):
            repo_part = repo_part[:-4]

        parts = repo_part.split("/")
        if len(parts) >= 2:
            return f"{parts[0]}/{parts[1]}"
        else:
            console.print(f"[red]Error: invalid GitHub URL format: {repo}[/red]")
            raise typer.Exit(1)
    elif "/" in repo:
        # Assume it's already in owner/repo format, remove .git if present
        if repo.endswith(".git"):
            repo = repo[:-4]
        return repo
    else:
        console.print("[red]Error: repository must be in 'owner/repo' format or full GitHub URL[/red]")
        console.print("[cyan]Examples: 'microsoft/vscode' or 'https://github.com/microsoft/vscode'[/cyan]")

        raise typer.Exit(1)


def _select_repo_from_list() -> Union[str, None]:
    """
    Show list of repositories and let user select one.
    Returns the full repository name (owner/repo).
    """
    console.print("[cyan]Fetching your repositories...[/cyan]")

    try:
        client = get_api_client()
        response = client.get(APIEndpoints.GITHUB_REPOSITORY)
        result = client.handle_response(response)
        if result is None:
            raise typer.Exit(1)

        repositories = result.get("repositories", [])
        total = result.get("total", 0)

        if not repositories:
            console.print("[yellow]No repositories available.[/yellow]")
            return None

    except Exception as e:
        console.print(f"[red]Error: could not fetch repositories: {e}[/red]")
        raise typer.Exit(1)

    console.print(f"\n[bold magenta]--- Select a repository to clone ({total} available) ---[/bold magenta]")
    
    for i, repo in enumerate(repositories, 1):
        repo_name = repo["full_name"]
        description = repo.get("description") or "No description"
        is_private = "🔒 Private" if repo["private"] else "🌐 Public"
        default_branch = repo.get("default_branch", "main")
        
        console.print(f"  [cyan]{i:2}.[/cyan] [green]{repo_name}[/green]")
        console.print(f"      [dim]{description}[/dim]")
        console.print(f"      [yellow]{is_private}[/yellow] • [blue]Branch: {default_branch}[/blue]")
        console.print()

    while True:
        try:
            choice_str = input(f"Enter the number of the repository to clone (1-{len(repositories)}): ")
            choice_index = int(choice_str) - 1
            if 0 <= choice_index < len(repositories):
                selected_repo = repositories[choice_index]
                return selected_repo["full_name"]
            else:
                console.print("[red]Invalid number. Please try again.[/red]")
        except ValueError:
            console.print("[red]Please enter a valid number.[/red]")
        except KeyboardInterrupt:
            console.print("\n[yellow]Operation cancelled.[/yellow]")
            raise typer.Exit(0)