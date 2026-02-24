"""
Image build and push command group for Nasiko Docker images.
Commands for building and pushing service images to a container registry.
"""

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Annotated, cast, Optional, List, Dict

import typer
from rich.console import Console
from rich.table import Table

console = Console()

# Create images command group
images_app = typer.Typer(help="Build and push Nasiko Docker images")

@dataclass(frozen=True)
class ImageSpec:
    dockerfile: str
    context: str
    aliases: tuple[str, ...] = ()


# Primary names here match the Kubernetes chart image repos under {{ PUBLIC_REGISTRY }}/<repo>:<tag>.
# Aliases keep older docker-compose / docs names working without duplicating builds.
SERVICES: dict[str, ImageSpec] = {
    "nasiko-backend": ImageSpec("core/app/Dockerfile", "core"),
    "nasiko-router": ImageSpec("core/agent-gateway/router/Dockerfile", "core/agent-gateway/router"),
    "nasiko-service-registry": ImageSpec(
        "core/agent-gateway/registry/Dockerfile",
        "core/agent-gateway/registry",
        aliases=("kong-service-registry",),
    ),
    "nasiko-chat-history-service": ImageSpec(
        "core/agent-gateway/chat-history-service/Dockerfile",
        "core/agent-gateway/chat-history-service",
        aliases=("chat-history-service",),
    ),
    "nasiko-auth": ImageSpec(
        "auth-service/Dockerfile",
        "auth-service",
        aliases=("nasiko-auth-service",),
    ),
    "nasiko-k8s-build-worker": ImageSpec(
        "core/app/Dockerfile.k8s-build-worker",
        "core",
        aliases=("nasiko-worker",),
    ),
    # Legacy orchestrator image (kept for backwards compatibility; not used by the K8s chart).
    "nasiko-orchestrator-worker": ImageSpec("core/Dockerfile.worker", "core"),
    "nasiko-web": ImageSpec("web/Dockerfile", "web"),
    # OSS single-user auth service image (explicit, opt-in).
    "nasiko-auth-oss": ImageSpec(
        "core/auth-service-oss/Dockerfile",
        "core/auth-service-oss",
        aliases=("nasiko-auth-service-oss",),
    ),
}


def _get_project_root() -> Path:
    """Get the repository root directory (parent of core/)."""
    current = Path(__file__).parent.parent.parent.parent  # cli/groups/../../.. = repo root
    if (current / "core").is_dir() and (current / "web").is_dir():
        return current
    raise FileNotFoundError("Could not find repository root (expected core/ and web/ directories)")


def _resolve_services(service_filter: Optional[List[str]]) -> Dict[str, ImageSpec]:
    """Validate and resolve service filter against known services (primary names or aliases)."""
    if not service_filter:
        return SERVICES

    name_to_primary: dict[str, str] = {primary: primary for primary in SERVICES}
    for primary, spec in SERVICES.items():
        for alias in spec.aliases:
            name_to_primary[alias] = primary

    resolved: dict[str, ImageSpec] = {}
    for name in service_filter:
        primary = name_to_primary.get(name)
        if not primary:
            console.print(f"[red]Unknown service: {name}[/]")
            console.print(f"Available services: {', '.join(SERVICES.keys())}")
            raise typer.Exit(1)
        resolved[primary] = SERVICES[primary]
    return resolved


def _docker_login_if_needed(username: str) -> None:
    """Check Docker login status and prompt login if needed."""
    config_path = Path.home() / ".docker" / "config.json"
    if config_path.exists():
        try:
            # Use cast to object to break the 'Any' chain from json.loads
            config_data = cast(object, json.loads(config_path.read_text()))
            if isinstance(config_data, dict):
                # Cast to a more specific dict type to avoid Unknown
                typed_config = cast(dict[str, object], config_data)
                auths_raw = typed_config.get("auths")
                if isinstance(auths_raw, dict):
                    # Cast to dict[str, object] to avoid Unknown keys
                    typed_auths = cast(dict[str, object], auths_raw)
                    # Check if logged into Docker Hub
                    for key in typed_auths:
                        if "docker.io" in key or "index.docker.io" in key:
                            return
        except (json.JSONDecodeError, OSError):
            pass

    console.print(f"[yellow]Not logged into Docker Hub. Running docker login...[/]")
    result = subprocess.run(
        ["docker", "login", "-u", username],
        check=False,
    )
    if result.returncode != 0:
        console.print("[red]Docker login failed.[/]")
        raise typer.Exit(1)


def _ensure_buildx() -> str:
    """Ensure Docker Buildx is available and return builder name."""
    # Check if buildx is available
    result = subprocess.run(
        ["docker", "buildx", "version"],
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        console.print("[red]Docker Buildx is not available.[/]")
        console.print("Please install Docker Desktop or enable Buildx plugin.")
        raise typer.Exit(1)

    # Check if a builder exists, create one if needed
    builder_name = "nasiko-multiplatform"
    result = subprocess.run(
        ["docker", "buildx", "inspect", builder_name],
        capture_output=True,
        check=False,
    )

    if result.returncode != 0:
        console.print(f"[yellow]Creating buildx builder: {builder_name}[/]")
        result = subprocess.run(
            ["docker", "buildx", "create", "--name", builder_name, "--use"],
            check=False,
        )
        if result.returncode != 0:
            console.print("[red]Failed to create buildx builder.[/]")
            raise typer.Exit(1)

    # Use the builder
    subprocess.run(["docker", "buildx", "use", builder_name], check=False, capture_output=True)
    return builder_name


def _build_images(
    username: str,
    tag: str,
    services: dict[str, ImageSpec],
    platform: str,
    no_cache: bool,
    dry_run: bool,
    push: bool = False,
) -> bool:
    """Build Docker images for the specified services. Returns True on success."""
    project_root = _get_project_root()
    success = True

    # Check if multi-platform build
    is_multiplatform = "," in platform

    # Ensure buildx is available for multi-platform builds
    if is_multiplatform and not dry_run:
        _ensure_buildx()

    for primary, spec in services.items():
        repos = (primary, *spec.aliases)
        images = [f"{username}/{repo}:{tag}" for repo in repos]
        dockerfile_path = project_root / spec.dockerfile
        context_path = project_root / spec.context

        if not dockerfile_path.exists():
            console.print(f"[red]Dockerfile not found: {dockerfile_path}[/]")
            success = False
            continue

        # Use buildx for multi-platform or regular docker build for single platform
        if is_multiplatform:
            cmd = [
                "docker", "buildx", "build",
                "-f", str(dockerfile_path),
                "--platform", platform,
            ]
            for img in images:
                cmd.extend(["-t", img])
            # Multi-platform builds require --push or --load
            # --load only works with single platform, so we use --push for multi-platform
            if push:
                cmd.append("--push")
            else:
                # For multi-platform without push, build and keep in cache
                console.print(f"[yellow]Note: Multi-platform build will be cached locally but not loaded to docker images[/]")
                console.print(f"[yellow]Use --push flag with build-push command to push to registry[/]")
        else:
            cmd = [
                "docker", "build",
                "-f", str(dockerfile_path),
                "--platform", platform,
            ]
            for img in images:
                cmd.extend(["-t", img])

        if no_cache:
            cmd.append("--no-cache")
        cmd.append(str(context_path))

        if dry_run:
            console.print(f"[dim][dry-run][/] {' '.join(cmd)}")
            continue

        console.print(f"[cyan]Building {images[0]} for {platform}...[/]")
        result = subprocess.run(cmd, check=False)
        if result.returncode != 0:
            console.print(f"[red]Failed to build {images[0]}[/]")
            success = False
        else:
            console.print(f"[green]Built {images[0]}[/]")

    return success


def _push_images(
    username: str,
    tag: str,
    services: dict[str, ImageSpec],
    dry_run: bool,
) -> bool:
    """Push Docker images for the specified services. Returns True on success."""
    success = True

    for primary, spec in services.items():
        repos = (primary, *spec.aliases)
        images = [f"{username}/{repo}:{tag}" for repo in repos]
        for image in images:
            cmd = ["docker", "push", image]

            if dry_run:
                console.print(f"[dim][dry-run][/] {' '.join(cmd)}")
                continue

            console.print(f"[cyan]Pushing {image}...[/]")
            result = subprocess.run(cmd, check=False)
            if result.returncode != 0:
                console.print(f"[red]Failed to push {image}[/]")
                success = False
            else:
                console.print(f"[green]Pushed {image}[/]")

    return success


@images_app.command(name="build")
def build_cmd(
    username: Annotated[
        str,
        typer.Option(
            "--username",
            "-u",
            envvar="NASIKO_PUBLIC_REGISTRY_USER",
            help="Docker Hub username or registry namespace (defaults to $NASIKO_PUBLIC_REGISTRY_USER)",
        ),
    ] = "karannasiko",
    tag: Annotated[str, typer.Option("--tag", "-t", help="Image tag")] = "latest",
    service: Annotated[Optional[List[str]], typer.Option("--service", "-s", help="Specific service(s) to build (repeatable)")] = None,
    platform: Annotated[str, typer.Option("--platform", help="Target platform(s), comma-separated for multi-platform")] = "linux/amd64",
    multi_platform: Annotated[bool, typer.Option("--multi-platform", help="Build for both amd64 and arm64")] = False,
    no_cache: Annotated[bool, typer.Option("--no-cache", help="Build without Docker cache")] = False,
    dry_run: Annotated[bool, typer.Option("--dry-run", help="Print commands without executing")] = False,
) -> None:
    """Build Docker images for Nasiko services."""
    services = _resolve_services(service)

    # Override platform if multi-platform flag is set
    if multi_platform:
        platform = "linux/amd64,linux/arm64"

    console.print(f"[bold]Building {len(services)} image(s) as {username}/<service>:{tag}[/]")
    console.print(f"[bold]Target platform(s): {platform}[/]\n")

    if not dry_run:
        # Verify docker is available
        result = subprocess.run(["docker", "version"], capture_output=True, check=False)
        if result.returncode != 0:
            console.print("[red]Docker is not running.[/]")
            raise typer.Exit(1)

    ok = _build_images(username, tag, services, platform, no_cache, dry_run, push=False)
    if not ok:
        raise typer.Exit(1)

    if not dry_run:
        console.print(f"\n[green]All images built successfully.[/]")


@images_app.command(name="push")
def push_cmd(
    username: Annotated[
        str,
        typer.Option(
            "--username",
            "-u",
            envvar="NASIKO_PUBLIC_REGISTRY_USER",
            help="Docker Hub username or registry namespace (defaults to $NASIKO_PUBLIC_REGISTRY_USER)",
        ),
    ] = "karannasiko",
    tag: Annotated[str, typer.Option("--tag", "-t", help="Image tag")] = "latest",
    service: Annotated[Optional[List[str]], typer.Option("--service", "-s", help="Specific service(s) to push (repeatable)")] = None,
    dry_run: Annotated[bool, typer.Option("--dry-run", help="Print commands without executing")] = False,
) -> None:
    """Push Docker images to the registry."""
    services = _resolve_services(service)

    console.print(f"[bold]Pushing {len(services)} image(s) as {username}/<service>:{tag}[/]\n")

    if not dry_run:
        _docker_login_if_needed(username)

    ok = _push_images(username, tag, services, dry_run)
    if not ok:
        raise typer.Exit(1)

    if not dry_run:
        console.print(f"\n[green]All images pushed successfully.[/]")


@images_app.command(name="build-push")
def build_push_cmd(
    username: Annotated[
        str,
        typer.Option(
            "--username",
            "-u",
            envvar="NASIKO_PUBLIC_REGISTRY_USER",
            help="Docker Hub username or registry namespace (defaults to $NASIKO_PUBLIC_REGISTRY_USER)",
        ),
    ] = "karannasiko",
    tag: Annotated[str, typer.Option("--tag", "-t", help="Image tag")] = "latest",
    service: Annotated[Optional[List[str]], typer.Option("--service", "-s", help="Specific service(s) (repeatable)")] = None,
    platform: Annotated[str, typer.Option("--platform", help="Target platform(s), comma-separated for multi-platform")] = "linux/amd64",
    multi_platform: Annotated[bool, typer.Option("--multi-platform", help="Build for both amd64 and arm64")] = False,
    no_cache: Annotated[bool, typer.Option("--no-cache", help="Build without Docker cache")] = False,
    dry_run: Annotated[bool, typer.Option("--dry-run", help="Print commands without executing")] = False,
) -> None:
    """Build and push Docker images for Nasiko services."""
    services = _resolve_services(service)

    # Override platform if multi-platform flag is set
    if multi_platform:
        platform = "linux/amd64,linux/arm64"

    console.print(f"[bold]Building and pushing {len(services)} image(s) as {username}/<service>:{tag}[/]")
    console.print(f"[bold]Target platform(s): {platform}[/]\n")

    if not dry_run:
        result = subprocess.run(["docker", "version"], capture_output=True, check=False)
        if result.returncode != 0:
            console.print("[red]Docker is not running.[/]")
            raise typer.Exit(1)
        _docker_login_if_needed(username)

    # For multi-platform, buildx handles both build and push
    is_multiplatform = "," in platform
    if is_multiplatform:
        ok = _build_images(username, tag, services, platform, no_cache, dry_run, push=True)
        if not ok:
            raise typer.Exit(1)
    else:
        # For single platform, build then push separately
        ok = _build_images(username, tag, services, platform, no_cache, dry_run, push=False)
        if not ok:
            raise typer.Exit(1)

        if not dry_run:
            console.print()

        ok = _push_images(username, tag, services, dry_run)
        if not ok:
            raise typer.Exit(1)

    if not dry_run:
        console.print(f"\n[green]All images built and pushed successfully.[/]")


@images_app.command(name="list")
def list_cmd() -> None:
    """List all services and their Dockerfiles."""
    table = Table(title="Nasiko Services")
    table.add_column("Service", style="cyan")
    table.add_column("Dockerfile", style="green")
    table.add_column("Build Context", style="yellow")
    table.add_column("Also Tags", style="dim")

    for name, spec in SERVICES.items():
        table.add_row(name, spec.dockerfile, spec.context, ", ".join(spec.aliases))

    console.print(table)
