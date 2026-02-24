"""
Main entry point for Nasiko CLI.
"""

from setup import setup
import os
import sys

import typer

# Add CLI directory to Python path once
current_dir = os.path.dirname(os.path.abspath(__file__))
if current_dir not in sys.path:
    sys.path.insert(0, current_dir)


def _load_env_file_early():
    """
    Load environment file at startup before Typer processes commands.

    This ensures that environment variables from .env files are available
    when Typer reads the envvar parameters.

    Searches for config files in order:
    1. Path specified by --config argument (if present)
    2. .nasiko.env in current directory
    3. .nasiko-aws.env in current directory
    4. .nasiko-do.env in current directory
    5. .env in current directory
    """
    from pathlib import Path

    def _load_simple_dotenv(path: Path, override: bool) -> None:
        """
        Minimal dotenv loader to avoid requiring python-dotenv at runtime.

        Supports:
        - whitespace around '='
        - single/double quoted values
        - optional 'export ' prefix
        - full-line comments (#...) and blank lines
        """
        try:
            data = path.read_text(encoding="utf-8")
        except Exception:
            return

        for raw in data.splitlines():
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("export "):
                line = line[len("export ") :].lstrip()

            if "=" not in line:
                continue
            k, v = line.split("=", 1)
            key = k.strip()
            if not key:
                continue

            val = v.strip()
            if not val:
                value = ""
            elif len(val) >= 2 and val[0] == val[-1] and val[0] in ("'", '"'):
                value = val[1:-1]
            else:
                # Strip inline comments for unquoted values.
                value = val.split("#", 1)[0].rstrip()

            if not override and key in os.environ:
                continue
            os.environ[key] = value

    def _load_dotenv_file(path: Path, override: bool) -> None:
        try:
            from dotenv import load_dotenv  # type: ignore
        except Exception:
            _load_simple_dotenv(path, override=override)
            return

        load_dotenv(path, override=override)

    # Check if --config/-c is specified in argv
    config_path = None
    for i, arg in enumerate(sys.argv):
        if arg in ("--config", "-c") and i + 1 < len(sys.argv):
            config_path = sys.argv[i + 1]
            break
        if arg.startswith("--config="):
            config_path = arg.split("=", 1)[1]
            break

    if config_path:
        path = Path(config_path)
        if path.exists():
            # Explicit config file should win over any exported env vars.
            _load_dotenv_file(path, override=True)
            return

    # Search for config files
    cwd = Path.cwd()
    search_paths = [
        ".nasiko-local.env",
        ".nasiko.env",
        ".nasiko-aws.env",
        ".nasiko-do.env",
        ".env",
    ]

    for filename in search_paths:
        path = cwd / filename
        if path.exists():
            _load_dotenv_file(path, override=False)
            return

# Create main CLI app
app = typer.Typer(
    help="Nasiko CLI - Build, deploy, and manage AI agents with ease"
)


def version_callback(value: bool):
    """Show version and exit."""
    if value:
        try:
            from importlib.metadata import version
            __version__ = version("nasiko-cli")
        except Exception:
            __version__ = "2.0.0"  # fallback

        typer.echo(f"Nasiko CLI v{__version__}")
        raise typer.Exit()


@app.callback()
def callback(
    version: bool = typer.Option(
        False, "--version", callback=version_callback, is_eager=True, help="Show version and exit"
    ),
    # NOTE: Do not use "-c" here. "-c" is reserved for config files in setup commands
    # (e.g. `nasiko setup bootstrap -c .hackathon.env`).
    cluster: str = typer.Option(None, "--cluster", "-n", help="Cluster name to use for this command")
):
    """Main CLI entry point."""
    if cluster:
        os.environ["NASIKO_CLUSTER_NAME"] = cluster
    pass


# Top-level Authentication Commands (for convenience)
@app.command(name="login")
def login_cmd(
    access_key: str = typer.Option(..., "--access-key", "-k", prompt="Access Key", help="Your access key"),
    access_secret: str = typer.Option(..., "--access-secret", "-s", prompt="Access Secret", hide_input=True, help="Your access secret"),
):
    """Login to Nasiko."""
    from auth.auth_commands import login_standalone
    login_standalone(access_key, access_secret)


@app.command(name="logout")
def logout_cmd():
    """Logout from Nasiko."""
    from auth.auth_commands import logout_command
    logout_command()


@app.command(name="status")
def auth_status_cmd():
    """Check authentication status."""
    from auth.auth_commands import status_command
    status_command()


@app.command(name="whoami")
def whoami_cmd():
    """Show current user information."""
    from auth.auth_commands import whoami_command
    whoami_command()

@app.command(name="docs")
def api_docs():
    """Get API documentation and Swagger links."""
    from commands.registry import api_docs_command
    api_docs_command()

@app.command(name="list-clusters")
def list_clusters_cmd():
    """List all configured Nasiko clusters."""
    from setup.config import list_clusters
    from rich.console import Console
    from rich.table import Table
    
    console = Console()
    clusters = list_clusters()
    
    if not clusters:
        console.print("[yellow]No clusters found.[/]")
        console.print("Use [bold]nasiko setup deploy[/] to create a new cluster.")
        return

    table = Table(title="Nasiko Clusters")
    table.add_column("Name", style="cyan", no_wrap=True)
    table.add_column("Provider", style="magenta")
    table.add_column("Gateway URL", style="green")
    
    for cluster in clusters:
        table.add_row(
            cluster.get("name", "Unknown"),
            cluster.get("provider", "Unknown"),
            cluster.get("url", "Unknown")
        )
        
    console.print(table)

app.add_typer(
    setup.app, 
    name="setup", 
    help="Setup Nasiko cluster components (registry, k8s, etc.)."
)


# Import and register command groups
def register_groups():
    """Register all command groups."""
    from groups.github_group import github_app
    from groups.agent_group import agent_app
    from groups.n8n_group import n8n_app
    from groups.chat_group import chat_app
    from groups.search_group import search_app
    from groups.observability_group import observability_app
    from groups.access_group import access_app
    from groups.user_group import user_app
    from groups.local_group import local_app
    from groups.images_group import images_app

    # Add groups to main app
    app.add_typer(github_app, name="github")
    app.add_typer(agent_app, name="agent")
    app.add_typer(n8n_app, name="n8n")
    app.add_typer(chat_app, name="chat")
    app.add_typer(search_app, name="search")
    app.add_typer(observability_app, name="observability")
    app.add_typer(access_app, name="access")
    app.add_typer(user_app, name="user")
    app.add_typer(local_app, name="local")
    app.add_typer(images_app, name="images")


def main():
    """CLI entry point for the app."""
    # Load environment files early, before Typer processes envvar parameters
    _load_env_file_early()
    register_groups()
    app()


if __name__ == "__main__":
    main()
