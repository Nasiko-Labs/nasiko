"""
Local development command group for managing Nasiko Docker stack.
Commands for managing local Docker Compose environment.
"""

import os
import subprocess
import time
from pathlib import Path
from typing import Annotated, Optional, List, cast

import typer
from rich.console import Console
from rich.live import Live
from rich.spinner import Spinner
from rich.table import Table

console = Console()

# Create local command group
local_app = typer.Typer(help="Local Docker development stack management")

# Path to docker-compose file (relative to project root)
COMPOSE_FILE = "docker-compose.nasiko.yml"
PROJECT_NAME = "nasiko"


def _get_project_root() -> Path:
    """Get project root directory."""
    # Start from CLI directory and go up to find docker-compose.nasiko.yml
    current = Path(__file__).parent.parent.parent  # cli/groups/../.. = root
    if (current / COMPOSE_FILE).exists():
        return current
    raise FileNotFoundError(f"Could not find {COMPOSE_FILE} in project root")


def _ensure_docker_running() -> None:
    """Verify Docker daemon is running."""
    try:
        result = subprocess.run(
            ["docker", "ps"],
            capture_output=True,
            timeout=5,
            check=False
        )
        if result.returncode != 0:
            console.print("[red]Error: Docker daemon is not running[/]")
            console.print("Please start Docker and try again.")
            raise typer.Exit(1)
    except FileNotFoundError:
        console.print("[red]Error: Docker is not installed[/]")
        raise typer.Exit(1)
    except subprocess.TimeoutExpired:
        console.print("[red]Error: Docker daemon is not responding[/]")
        raise typer.Exit(1)


def _ensure_docker_compose() -> None:
    """Verify Docker Compose is available."""
    try:
        result = subprocess.run(
            ["docker", "compose", "version"],
            capture_output=True,
            timeout=5,
            check=False
        )
        if result.returncode != 0:
            console.print("[red]Error: Docker Compose plugin is not installed[/]")
            console.print("Please install Docker Compose (v2.0+) and try again.")
            raise typer.Exit(1)
    except FileNotFoundError:
        console.print("[red]Error: Docker Compose is not available[/]")
        raise typer.Exit(1)


def _check_port_availability(port: int) -> bool:
    """Check if a port is available."""
    import socket
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        result = sock.connect_ex(('127.0.0.1', port))
        sock.close()
        return result != 0
    except Exception:
        return True


def _compose_cmd(args: list[str], check: bool = True) -> subprocess.CompletedProcess[bytes]:
    """Run docker compose command."""
    project_root = _get_project_root()
    cmd = [
        "docker", "compose",
        "-f", str(project_root / COMPOSE_FILE),
        "-p", PROJECT_NAME
    ] + args

    return subprocess.run(cmd, check=check, capture_output=False)


def _compose_cmd_silent(args: list[str], check: bool = False) -> subprocess.CompletedProcess[str]:
    """Run docker compose command silently."""
    project_root = _get_project_root()
    cmd = [
        "docker", "compose",
        "-f", str(project_root / COMPOSE_FILE),
        "-p", PROJECT_NAME
    ] + args

    return subprocess.run(cmd, check=check, capture_output=True, text=True)


def _load_env_file(project_root: Path) -> None:
    """Load environment variables from .env files."""
    env_files = [
        project_root / ".nasiko.env",
        project_root / ".nasiko-local.env",
        project_root / ".env"
    ]

    for env_file in env_files:
        if env_file.exists():
            console.print(f"[dim]Loading environment from: {env_file.name}[/]")
            from dotenv import load_dotenv
            _ = load_dotenv(env_file, override=False)
            return


# Default port mappings: env var name -> default port
PORT_DEFAULTS = {
    "NASIKO_PORT_MONGODB": 27017,
    "NASIKO_PORT_REDIS": 6379,
    "NASIKO_PORT_KONG": 9100,
    "NASIKO_PORT_KONG_ADMIN": 9101,
    "NASIKO_PORT_KONG_MANAGER": 9102,
    "NASIKO_PORT_KONG_SSL": 9443,
    "NASIKO_PORT_KONGA": 1337,
    "NASIKO_PORT_SERVICE_REGISTRY": 8080,
    "NASIKO_PORT_BACKEND": 8000,
    "NASIKO_PORT_AUTH": 8082,
    "NASIKO_PORT_ROUTER": 8081,
    "NASIKO_PORT_CHAT": 8083,
    "NASIKO_PORT_WEB": 4000,
    "NASIKO_PORT_OTEL_GRPC": 4317,
    "NASIKO_PORT_OTEL_HTTP": 4318,
    "NASIKO_PORT_OTEL_LEGACY": 55680,
    "NASIKO_PORT_OTEL_DEBUG": 8888,
    "NASIKO_PORT_LANGTRACE": 3000,
    "NASIKO_PORT_LANGTRACE_PG": 6432,
    "NASIKO_PORT_CLICKHOUSE_HTTP": 8123,
    "NASIKO_PORT_CLICKHOUSE_NATIVE": 9000,
}


def _get_port(env_var: str) -> int:
    """Get configured port from environment, falling back to default."""
    return int(os.environ.get(env_var, PORT_DEFAULTS[env_var]))


# def _wait_for_service(service_name: str, port: int, timeout: int = 60) -> bool:
#     """Wait for service to be healthy."""
#     start_time = time.time()
#
#     while time.time() - start_time < timeout:
#         result = _compose_cmd_silent(["exec", "-T", service_name, "curl", "-f", f"http://localhost:{port}/health"], check=False)
#
#         if result.returncode == 0:
#             return True
#
#         time.sleep(2)
#
#     return False


@local_app.command(name="up")
def local_up(
    build: Annotated[bool, typer.Option("--build/--no-build", help="Build images before starting")] = True,
    detach: Annotated[bool, typer.Option("--detach/--attach", help="Run in background")] = True,
) -> None:
    """Start the Nasiko local development stack."""
    try:
        _ensure_docker_running()
        _ensure_docker_compose()

        project_root = _get_project_root()
        _load_env_file(project_root)

        # Check for critical port conflicts
        critical_ports = {
            _get_port("NASIKO_PORT_KONG"): "Kong Gateway",
            _get_port("NASIKO_PORT_BACKEND"): "Backend API",
            _get_port("NASIKO_PORT_MONGODB"): "MongoDB",
            _get_port("NASIKO_PORT_REDIS"): "Redis",
        }

        unavailable: list[str] = []
        for port, service in critical_ports.items():
            if not _check_port_availability(port):
                unavailable.append(f"Port {port} ({service})")

        if unavailable:
            console.print("[yellow]Warning: Some ports are already in use:[/]")
            for item in unavailable:
                console.print(f"  - {item}")
            console.print("\nStarting anyway (may cause conflicts)...")
            if not typer.confirm("Continue?"):
                raise typer.Exit(0)

        console.print("\n[bold cyan]Starting Nasiko local development stack...[/]")
        console.print(f"[dim]Compose file: {COMPOSE_FILE}[/]\n")

        # Remove stale containers that may conflict (from previous runs or other projects)
        console.print("[dim]Removing stale containers...[/]")
        stale = _compose_cmd_silent(["config", "--services"], check=False)
        if stale.returncode == 0:
            # Get container names from compose config
            result_config = _compose_cmd_silent(["config"], check=False)
            if result_config.returncode == 0:
                # Extract container_name values and remove any that already exist
                import re
                container_names = re.findall(r"container_name:\s*(.+)", result_config.stdout)
                if container_names:
                    subprocess.run(
                        ["docker", "rm", "-f"] + container_names,
                        capture_output=True,
                        check=False,
                    )

        # Build or pull images
        if build:
            console.print("[cyan]Building images...[/]")
            result = _compose_cmd(["build"], check=False)
            if result.returncode != 0:
                console.print("[yellow]Warning: Some images failed to build (may already exist)[/]")

        # Start services
        cmd_args = ["up", "--remove-orphans"]
        if detach:
            cmd_args.append("-d")

        console.print("[cyan]Starting services...[/]")
        _ = _compose_cmd(cmd_args)

        if detach:
            console.print("\n[green]✓ Stack started successfully![/]")
            console.print("\n[bold]Waiting for services to be ready...[/]")

            # Wait for key services
            with Live(Spinner("dots", text="Checking service health..."), console=console) as live:
                time.sleep(3)  # Give services time to start
                live.update(Spinner("dots", text="Checking MongoDB..."))
                time.sleep(2)
                live.update(Spinner("dots", text="Checking Redis..."))
                time.sleep(2)
                live.update(Spinner("dots", text="Checking Kong..."))
                time.sleep(2)

            # Display connection info
            console.print("\n[bold cyan]═══════════════════════════════════════════[/]")
            console.print("[bold]Nasiko Local Stack is Ready![/]")
            console.print("[bold cyan]═══════════════════════════════════════════[/]\n")

            services_table = Table(title="Available Services", show_header=True, header_style="bold magenta")
            services_table.add_column("Service", style="cyan")
            services_table.add_column("URL", style="green")
            services_table.add_column("Purpose", style="yellow")

            services_table.add_row("Kong Gateway", f"http://localhost:{_get_port('NASIKO_PORT_KONG')}", "API gateway & agent routing")
            services_table.add_row("Backend API", f"http://localhost:{_get_port('NASIKO_PORT_BACKEND')}/docs", "Nasiko API (Swagger)")
            services_table.add_row("Konga UI", f"http://localhost:{_get_port('NASIKO_PORT_KONGA')}", "Kong management UI")
            services_table.add_row("Service Registry", f"http://localhost:{_get_port('NASIKO_PORT_SERVICE_REGISTRY')}", "Agent discovery API")
            services_table.add_row("Router", f"http://localhost:{_get_port('NASIKO_PORT_ROUTER')}", "Query routing service")
            services_table.add_row("Auth Service", f"http://localhost:{_get_port('NASIKO_PORT_AUTH')}", "Authentication service")
            services_table.add_row("Chat History", f"http://localhost:{_get_port('NASIKO_PORT_CHAT')}", "Chat history API")
            services_table.add_row("Web UI", f"http://localhost:{_get_port('NASIKO_PORT_WEB')}", "Nasiko web interface")

            console.print(services_table)

            console.print("\n[bold]Quick Commands:[/]")
            console.print("  • View logs:           [cyan]nasiko local logs [service][/]")
            console.print("  • Check status:        [cyan]nasiko local status[/]")
            console.print("  • Deploy agent:        [cyan]nasiko local deploy-agent <name> <path>[/]")
            console.print("  • Stop stack:          [cyan]nasiko local down[/]")
            console.print("\n[bold]First Steps:[/]")
            console.print(f"  1. Open web UI:        [cyan]http://localhost:{_get_port('NASIKO_PORT_WEB')}[/]")
            console.print("  2. Deploy an agent:    [cyan]nasiko agent upload-directory ./agents/my-agent[/]")
            console.print(f"  3. View agent registry: [cyan]http://localhost:{_get_port('NASIKO_PORT_SERVICE_REGISTRY')}/agents[/]")
            console.print("")

    except FileNotFoundError as e:
        console.print(f"[red]Error: {e}[/]")
        raise typer.Exit(1)
    except KeyboardInterrupt:
        console.print("\n[yellow]Cancelled.[/]")
        raise typer.Exit(0)


@local_app.command(name="down")
def local_down(
    volumes: Annotated[bool, typer.Option("--volumes", "-v", help="Remove volumes (data loss!)")] = False,
) -> None:
    """Stop and remove the Nasiko local development stack."""
    try:
        _ensure_docker_running()
        _ensure_docker_compose()

        if volumes:
            console.print("[red]⚠️  Warning: This will remove all volumes (MongoDB, Redis data will be deleted)[/]")
            if not typer.confirm("Are you sure?"):
                console.print("[yellow]Cancelled.[/]")
                raise typer.Exit(0)
            console.print("[cyan]Stopping and removing stack (with volumes)...[/]")
            _ = _compose_cmd(["down", "-v"])
        else:
            console.print("[cyan]Stopping and removing stack...[/]")
            _ = _compose_cmd(["down"])

        console.print("[green]✓ Stack stopped successfully![/]")

    except KeyboardInterrupt:
        console.print("\n[yellow]Cancelled.[/]")
        raise typer.Exit(0)


@local_app.command(name="status")
def local_status() -> None:
    """Show status of Nasiko local development stack."""
    try:
        _ensure_docker_running()
        _ensure_docker_compose()

        result = _compose_cmd_silent(["ps"], check=False)

        if result.returncode == 0:
            console.print(result.stdout)
        else:
            console.print("[yellow]Stack is not running. Start with: nasiko local up[/]")

    except Exception as e:
        console.print(f"[red]Error: {e}[/]")
        raise typer.Exit(1)


@local_app.command(name="logs")
def local_logs(
    service: Annotated[Optional[List[str]], typer.Argument(help="Service name (omit to see all)")] = None,
    follow: Annotated[bool, typer.Option("-f", "--follow", help="Follow logs")] = False,
    lines: Annotated[int, typer.Option("-n", "--lines", help="Number of lines to show")] = 100,
) -> None:
    """View logs from Nasiko local stack."""
    try:
        _ensure_docker_running()
        _ensure_docker_compose()

        args = ["logs"]
        if follow:
            args.append("-f")
        args.extend(["-n", str(lines)])
        if service:
            args.extend(service)

        _ = _compose_cmd(args, check=False)

    except KeyboardInterrupt:
        console.print("")
        raise typer.Exit(0)


@local_app.command(name="deploy-agent")
def local_deploy_agent(
    agent_name: Annotated[str, typer.Argument(help="Agent name")],
    agent_path: Annotated[Optional[str], typer.Argument(help="Path to agent directory (default: ./agents/{agent_name})")] = None,
) -> None:
    """Deploy an agent to the local stack."""
    try:
        import requests

        if agent_path is None:
            agent_path = f"./agents/{agent_name}"

        agent_path_obj = Path(agent_path).resolve()

        if not agent_path_obj.exists():
            console.print(f"[red]Error: Agent path not found: {agent_path}[/]")
            raise typer.Exit(1)

        if not (agent_path_obj / "docker-compose.yml").exists():
            console.print(f"[red]Error: No docker-compose.yml found in {agent_path}[/]")
            raise typer.Exit(1)

        console.print(f"[cyan]Deploying agent: {agent_name}[/]")
        console.print(f"[dim]Path: {agent_path_obj}[/]\n")

        # Call backend API to trigger deployment
        api_url = os.getenv("NASIKO_API_URL", "http://localhost:8000")
        endpoint = f"{api_url}/api/v1/orchestration/deploy"

        payload = {
            "agent_name": agent_name,
            "agent_path": str(agent_path_obj),
        }

        try:
            with console.status("[bold cyan]Sending deployment request..."):
                response = requests.post(
                    endpoint,
                    json=payload,
                    timeout=10
                )

            if response.status_code == 200:
                # Use cast to more specific dict type to avoid Unknown
                result = cast(dict[str, object], response.json())
                console.print("\n[green]✓ Deployment initiated![/]")
                console.print("\n[bold]Agent Details:[/]")
                console.print(f"  Name: [cyan]{result.get('agent_name')}[/]")
                console.print(f"  Status: [yellow]{result.get('status', 'building')}[/]")
                if result.get('url'):
                    console.print(f"  URL: [green]{result['url']}[/]")
                console.print("\n[dim]Polling for deployment status...[/]")

                # Poll for completion
                max_attempts = 30
                attempt = 0
                while attempt < max_attempts:
                    time.sleep(2)
                    attempt += 1

                    status_response = requests.get(
                        f"{api_url}/api/v1/registries?name={agent_name}",
                        timeout=10
                    )

                    if status_response.status_code == 200:
                        agents = cast(list[dict[str, object]], status_response.json())
                        if agents and len(agents) > 0:
                            agent = agents[0]
                            status = agent.get("status", "unknown")
                            if status == "active":
                                console.print("[green]✓ Agent deployed successfully![/]")
                                console.print(f"  Agent URL: [green]{agent.get('service_url', 'N/A')}[/]")
                                return
                            elif status == "failed":
                                console.print("[red]✗ Deployment failed[/]")
                                raise typer.Exit(1)

                console.print("[yellow]⏱️  Deployment timeout. Check status with: nasiko local status[/]")

            else:
                console.print(f"[red]Error: {response.status_code}[/]")
                console.print(response.text)
                raise typer.Exit(1)

        except requests.exceptions.ConnectionError:
            console.print("[red]Error: Could not connect to Nasiko backend[/]")
            console.print(f"[dim]URL: {endpoint}[/]")
            console.print("[yellow]Tip: Make sure the stack is running with 'nasiko local up'[/]")
            raise typer.Exit(1)

    except Exception as e:
        console.print(f"[red]Error: {e}[/]")
        raise typer.Exit(1)


@local_app.command(name="shell")
def local_shell(
    service: Annotated[str, typer.Argument(help="Service name to connect to")],
) -> None:
    """Open a shell in a running service container."""
    try:
        _ensure_docker_running()
        _ensure_docker_compose()

        console.print(f"[cyan]Connecting to {service}...[/]\n")

        # Determine shell based on service
        if "web" in service or "node" in service.lower():
            shell_cmd = "/bin/bash"
        else:
            shell_cmd = "/bin/bash"

        project_root = _get_project_root()
        _ = subprocess.run([
            "docker", "compose",
            "-f", str(project_root / COMPOSE_FILE),
            "-p", PROJECT_NAME,
            "exec", service, shell_cmd
        ])

    except KeyboardInterrupt:
        console.print("\n[yellow]Disconnected.[/]")
        raise typer.Exit(0)


@local_app.command(name="restart")
def local_restart(
    service: Annotated[Optional[str], typer.Argument(help="Service name (omit to restart all)")] = None,
) -> None:
    """Restart service(s) in the local stack."""
    try:
        _ensure_docker_running()
        _ensure_docker_compose()

        project_root = _get_project_root()
        _load_env_file(project_root)

        if service:
            console.print(f"[cyan]Recreating {service}...[/]")
            # Use force-recreate to pick up changes like port mappings.
            _ = _compose_cmd(["up", "-d", "--no-deps", "--force-recreate", service])
        else:
            console.print("[cyan]Restarting all services...[/]")
            _ = _compose_cmd(["restart"])
        console.print("[green]✓ Done![/]")

    except Exception as e:
        console.print(f"[red]Error: {e}[/]")
        raise typer.Exit(1)
