"""
Cluster command group.

Maps new `nasiko cluster *` surface to existing setup module implementations.
"""

from typing import Optional
import typer

cluster_app = typer.Typer(help="Cluster lifecycle — create, inspect, and destroy clusters")
create_app = typer.Typer(help="Create a new cluster", no_args_is_help=True)
setup_app = typer.Typer(help="Deploy cluster components", no_args_is_help=True)
registry_app = typer.Typer(help="Setup container registry", no_args_is_help=True)

cluster_app.add_typer(create_app, name="create")
cluster_app.add_typer(setup_app, name="setup")
setup_app.add_typer(registry_app, name="registry")


# ---------------------------------------------------------------------------
# nasiko cluster connect
# ---------------------------------------------------------------------------

@cluster_app.command("connect")
def connect(
    name: str = typer.Argument(..., help="Name to give this cluster"),
    url: str = typer.Option(..., "--url", "-u", help="API gateway URL (e.g. https://api.my-cluster.example.com)"),
    login: bool = typer.Option(True, "--login/--no-login", help="Prompt for login after connecting"),
    auth: str = typer.Option("key", "--auth", "-a", help="Login method: key, github, digitalocean"),
):
    """Connect to an existing remote cluster by registering its API URL."""
    import os
    from rich.console import Console
    from setup.config import save_cluster_info
    from core.context import set_active_cluster
    from auth.auth_manager import get_auth_manager

    console = Console()

    # Validate URL format
    if not url.startswith(("http://", "https://")):
        console.print("[red]URL must start with http:// or https://[/]")
        raise typer.Exit(1)

    if auth not in ("key", "github", "digitalocean"):
        console.print("[red]--auth must be one of: key, github, digitalocean[/]")
        raise typer.Exit(1)

    # Save cluster info
    save_cluster_info(
        provider="existing",
        cluster_name=name,
        data={"gateway_url": url.rstrip("/"), "type": "remote"},
    )
    set_active_cluster(name)
    # Propagate to env so subsequent auth calls use the right cluster
    os.environ["NASIKO_CLUSTER_NAME"] = name

    console.print(f"[green]Connected to cluster:[/] [bold]{name}[/]")
    console.print(f"[bold]API URL:[/] {url.rstrip('/')}")

    if not login:
        return

    console.print()

    if auth == "key":
        access_key = typer.prompt("Access key")
        access_secret = typer.prompt("Access secret", hide_input=True)
        auth_manager = get_auth_manager(cluster_name=name)
        if auth_manager.login(access_key, access_secret):
            console.print("[green]Login successful![/] You're ready to use Nasiko.")
        else:
            console.print("[yellow]Login failed.[/] Try [bold]nasiko auth login[/] to retry.")

    elif auth == "github":
        from commands.github import login_command
        console.print("[cyan]Logging in via GitHub SSO...[/cyan]")
        login_command()

    elif auth == "digitalocean":
        console.print("[cyan]DigitalOcean SSO is not yet supported.[/cyan]")
        console.print("Use [bold]nasiko auth login[/bold] with your access key instead.")


# ---------------------------------------------------------------------------
# nasiko cluster create
# ---------------------------------------------------------------------------

@create_app.command("local")
def create_local(
    name: str = typer.Option("local", "--name", "-n", help="Name for the cluster"),
):
    """Create a local Docker Compose cluster."""
    from groups.local_group import _ensure_docker_running, _ensure_docker_compose, local_up
    from setup.config import save_cluster_info
    from core.context import set_active_cluster

    _ensure_docker_running()
    _ensure_docker_compose()
    local_up()

    save_cluster_info(
        provider="local",
        cluster_name=name,
        data={"gateway_url": "http://localhost:9100", "type": "docker-compose"},
    )
    set_active_cluster(name)
    typer.echo(f"Active cluster set to: {name}")


@create_app.command("local-k8s")
def create_local_k8s(
    name: str = typer.Option("local-k8s", "--name", "-n", help="Name for the cluster"),
):
    """Create a local Kubernetes cluster via kind/minikube."""
    typer.echo("⚠️  Local Kubernetes cluster creation is not yet implemented.")
    typer.echo("Coming in a future release.")
    raise typer.Exit(1)


@create_app.command("remote")
def create_remote(
    provider: str = typer.Option(
        ..., "--provider", help="Cloud provider: aws or digitalocean"
    ),
    name: str = typer.Option(
        "nasiko", "--name", "-n", help="Name for the Kubernetes cluster"
    ),
    region: str = typer.Option(None, help="Cloud region (e.g. us-east-1, nyc3)"),
    node_size: str = typer.Option(None, help="Node instance type"),
    yes: bool = typer.Option(False, "--yes", "-y", help="Auto-approve Terraform apply"),
    verbose: bool = typer.Option(False, "--verbose", "-v", help="Verbose Terraform output"),
    terraform_dir: str = typer.Option(
        None, "--terraform-dir", "-t", envvar="NASIKO_TERRAFORM_DIR",
        help="Path to Terraform modules directory"
    ),
    state_dir: str = typer.Option(
        None, "--state-dir", envvar="NASIKO_STATE_DIR",
        help="Path for storing Terraform state"
    ),
):
    """Provision a remote Kubernetes cluster via Terraform (AWS or DigitalOcean)."""
    from setup.k8s_setup import create, Provider
    create(
        provider=Provider(provider),
        cluster_name=name,
        region=region,
        node_size=node_size,
        auto_approve=yes,
        verbose=verbose,
        terraform_dir=terraform_dir,
        state_dir=state_dir,
    )


# ---------------------------------------------------------------------------
# nasiko cluster destroy / list / output / state-info / init-modules
# ---------------------------------------------------------------------------

@cluster_app.command("destroy")
def destroy(
    name: str = typer.Argument(..., help="Name of the cluster to destroy"),
    provider: str = typer.Option(
        ..., "--provider", help="Cloud provider: aws or digitalocean"
    ),
    yes: bool = typer.Option(False, "--yes", "-y", help="Auto-approve Terraform destroy"),
    verbose: bool = typer.Option(False, "--verbose", "-v", help="Verbose Terraform output"),
    cleanup: bool = typer.Option(False, "--cleanup", help="Remove local state after destroy"),
    terraform_dir: str = typer.Option(
        None, "--terraform-dir", "-t", envvar="NASIKO_TERRAFORM_DIR"
    ),
    state_dir: str = typer.Option(
        None, "--state-dir", "-s", envvar="NASIKO_STATE_DIR"
    ),
):
    """Tear down a cluster and all related resources."""
    from setup.k8s_setup import destroy as _destroy, Provider
    _destroy(
        provider=Provider(provider),
        cluster_name=name,
        auto_approve=yes,
        verbose=verbose,
        terraform_dir=terraform_dir,
        state_dir=state_dir,
        cleanup_state=cleanup,
    )


@cluster_app.command("list")
def list_clusters():
    """List all clusters managed by Nasiko."""
    from rich.console import Console
    from rich.table import Table
    from setup.config import list_clusters as _list_config
    from core.context import get_active_cluster

    console = Console()
    clusters = _list_config()
    active = get_active_cluster()

    if not clusters:
        console.print("[yellow]No clusters found.[/yellow]")
        console.print("Create one with: [cyan]nasiko init[/cyan] or [cyan]nasiko cluster connect <name> --url <url>[/cyan]")
        return

    table = Table(title="Clusters")
    table.add_column("Name", style="bold")
    table.add_column("Provider")
    table.add_column("URL")
    table.add_column("Active")

    for c in clusters:
        is_active = "✓" if c.get("name") == active else ""
        table.add_row(c.get("name", ""), c.get("provider", ""), c.get("url", ""), f"[green]{is_active}[/green]")

    console.print(table)


@cluster_app.command("output")
def output(
    provider: str = typer.Option(..., "--provider", help="Cloud provider: aws or digitalocean"),
    name: str = typer.Option("nasiko", "--name", "-n", help="Cluster name"),
    terraform_dir: str = typer.Option(
        None, "--terraform-dir", "-t", envvar="NASIKO_TERRAFORM_DIR"
    ),
    state_dir: str = typer.Option(
        None, "--state-dir", "-s", envvar="NASIKO_STATE_DIR"
    ),
):
    """Show Terraform outputs for an existing cluster."""
    from setup.k8s_setup import output as _output, Provider
    _output(
        provider=Provider(provider),
        cluster_name=name,
        terraform_dir=terraform_dir,
        state_dir=state_dir,
    )


@cluster_app.command("state-info")
def state_info(
    provider: str = typer.Option(..., "--provider", help="Cloud provider: aws or digitalocean"),
    name: str = typer.Option("nasiko", "--name", "-n", help="Cluster name"),
    state_dir: str = typer.Option(
        None, "--state-dir", "-s", envvar="NASIKO_STATE_DIR"
    ),
):
    """Show detailed Terraform state info for a cluster."""
    from setup.k8s_setup import state_info as _state_info, Provider
    _state_info(
        provider=Provider(provider),
        cluster_name=name,
        state_dir=state_dir,
    )


@cluster_app.command("init-modules")
def init_modules(
    source: str = typer.Option(None, "--source", "-s", help="Source directory for Terraform modules"),
    force: bool = typer.Option(False, "--force", "-f", help="Overwrite existing modules"),
):
    """Copy Terraform modules to ~/.nasiko/terraform/."""
    from setup.k8s_setup import init_modules as _init_modules
    _init_modules(source=source, force=force)


# ---------------------------------------------------------------------------
# nasiko cluster bootstrap / configure-github-oauth / cleanup /
#             init-superuser / get-superuser
# ---------------------------------------------------------------------------

@cluster_app.command("bootstrap")
def bootstrap(
    config: str = typer.Option(None, "--config", "-c", help="Path to .env config file"),
    kubeconfig: str = typer.Option(None, envvar="KUBECONFIG", help="Existing kubeconfig (skips provisioning)"),
    provider: str = typer.Option(None, envvar="NASIKO_PROVIDER", help="Cloud provider: aws or digitalocean"),
    cluster_name: str = typer.Option("nasiko-cluster", envvar="NASIKO_CLUSTER_NAME", help="Cluster name"),
    region: str = typer.Option(None, envvar="NASIKO_REGION", help="Cloud region"),
    terraform_dir: str = typer.Option(None, "--terraform-dir", "-t", envvar="NASIKO_TERRAFORM_DIR"),
    state_dir: str = typer.Option(None, "--state-dir", envvar="NASIKO_STATE_DIR"),
    registry_type: str = typer.Option("harbor", envvar="NASIKO_CONTAINER_REGISTRY_TYPE", help="harbor or cloud"),
    domain: str = typer.Option(None, envvar="NASIKO_DOMAIN"),
    email: str = typer.Option(None, envvar="NASIKO_EMAIL"),
    registry_user: str = typer.Option("admin", envvar="NASIKO_REGISTRY_USER"),
    registry_pass: str = typer.Option(None, envvar="NASIKO_REGISTRY_PASS"),
    cloud_reg_name: str = typer.Option("nasiko-images", envvar="NASIKO_CONTAINER_REGISTRY_NAME"),
    openai_key: str = typer.Option(None, envvar="OPENAI_API_KEY"),
    public_registry_user: str = typer.Option("karannasiko", envvar="NASIKO_PUBLIC_REGISTRY_USER"),
    superuser_username: str = typer.Option("admin", envvar="NASIKO_SUPERUSER_USERNAME"),
    superuser_email: str = typer.Option("admin@nasiko.com", envvar="NASIKO_SUPERUSER_EMAIL"),
    clean_existing: bool = typer.Option(True, "--clean-existing/--no-clean-existing"),
):
    """Provision cluster + registry + buildkit + core apps in one shot."""
    from setup.setup import bootstrap as _bootstrap, RegistryType
    from setup.k8s_setup import Provider
    _bootstrap(
        config=config,
        kubeconfig=kubeconfig,
        provider=Provider(provider) if provider else None,
        cluster_name=cluster_name,
        region=region,
        terraform_dir=terraform_dir,
        state_dir=state_dir,
        registry_type=RegistryType(registry_type),
        domain=domain,
        email=email,
        registry_user=registry_user,
        registry_pass=registry_pass,
        cloud_reg_name=cloud_reg_name,
        openai_key=openai_key,
        public_registry_user=public_registry_user,
        superuser_username=superuser_username,
        superuser_email=superuser_email,
        clean_existing=clean_existing,
    )


@cluster_app.command("configure-github-oauth")
def configure_github_oauth(
    config: str = typer.Option(None, "--config", "-c"),
    kubeconfig: str = typer.Option(None, envvar="KUBECONFIG"),
    namespace: str = typer.Option("nasiko"),
    deployment: str = typer.Option("nasiko-backend"),
    container: str = typer.Option(None),
    restart: bool = typer.Option(True, "--restart/--no-restart"),
):
    """Patch GitHub OAuth env vars without re-bootstrapping."""
    from setup.setup import configure_github_oauth as _fn
    _fn(
        config=config,
        kubeconfig=kubeconfig,
        namespace=namespace,
        deployment=deployment,
        container=container,
        restart=restart,
    )


@cluster_app.command("cleanup")
def cleanup(
    kubeconfig: str = typer.Option(..., envvar="KUBECONFIG", help="Kubeconfig for the cluster to clean"),
    yes: bool = typer.Option(False, "--yes", "-y", help="Auto-approve cleanup"),
):
    """Remove all Nasiko resources from a cluster (namespaces, Helm releases)."""
    from setup.setup import cleanup as _cleanup
    _cleanup(kubeconfig=kubeconfig, auto_approve=yes)


@cluster_app.command("init-superuser")
def init_superuser(
    kubeconfig: str = typer.Option(None, envvar="KUBECONFIG"),
    superuser_username: str = typer.Option("admin", envvar="NASIKO_SUPERUSER_USERNAME"),
    superuser_email: str = typer.Option("admin@nasiko.com", envvar="NASIKO_SUPERUSER_EMAIL"),
    provider: str = typer.Option(None, envvar="NASIKO_PROVIDER"),
):
    """Create/recreate the super user and retrieve credentials."""
    from setup.setup import init_superuser as _fn
    from setup.k8s_setup import Provider
    _fn(
        kubeconfig=kubeconfig,
        superuser_username=superuser_username,
        superuser_email=superuser_email,
        provider=Provider(provider) if provider else None,
    )


@cluster_app.command("get-superuser")
def get_superuser(
    kubeconfig: str = typer.Option(None, envvar="KUBECONFIG"),
    provider: str = typer.Option(None, envvar="NASIKO_PROVIDER"),
    save: bool = typer.Option(True, "--save/--no-save"),
):
    """Fetch existing super user credentials (read-only)."""
    from setup.setup import get_superuser as _fn
    from setup.k8s_setup import Provider
    _fn(
        kubeconfig=kubeconfig,
        provider=Provider(provider) if provider else None,
        save_to_file=save,
    )


# ---------------------------------------------------------------------------
# nasiko cluster setup registry harbor / cloud
# nasiko cluster setup buildkit
# nasiko cluster setup core
# ---------------------------------------------------------------------------

@registry_app.command("harbor")
def setup_registry_harbor(
    domain: str = typer.Option(None, help="Domain for Harbor (e.g. reg.example.com)"),
    email: str = typer.Option(None, help="Email for Let's Encrypt"),
    password: str = typer.Option(..., help="Harbor admin password"),
    username: str = typer.Option("admin", help="Harbor admin username"),
):
    """Deploy Harbor container registry via Helm."""
    from setup.harbor_setup import deploy
    deploy(domain=domain, email=email, password=password, username=username)


@registry_app.command("cloud")
def setup_registry_cloud(
    provider: str = typer.Option(..., "--provider", help="aws or digitalocean"),
    region: str = typer.Option(None, help="Region (required for AWS)"),
    name: str = typer.Option(..., "--name", "-n", help="ECR repo or DO registry name"),
):
    """Setup ECR or DigitalOcean container registry."""
    from setup.container_registry_setup import deploy
    deploy(provider=provider, region=region, name=name)


@setup_app.command("buildkit")
def setup_buildkit(
    registry: str = typer.Option(..., help="Registry URL"),
    username: str = typer.Option(None, help="Registry username"),
    password: str = typer.Option(None, help="Registry password"),
    iam_role_arn: str = typer.Option(None, help="AWS IAM Role ARN for IRSA"),
):
    """Deploy rootless BuildKit to the cluster."""
    from setup.buildkit_setup import deploy
    deploy(registry=registry, username=username, password=password, iam_role_arn=iam_role_arn)


@setup_app.command("core")
def setup_core(
    registry_url: str = typer.Option(..., help="Registry URL"),
    registry_user: str = typer.Option(None),
    registry_pass: str = typer.Option(None),
    public_user: str = typer.Option("karannasiko", envvar="NASIKO_PUBLIC_REGISTRY_USER"),
    openai_key: str = typer.Option(None, envvar="OPENAI_API_KEY"),
    environment: str = typer.Option("default"),
    superuser_username: str = typer.Option("admin", envvar="NASIKO_SUPERUSER_USERNAME"),
    superuser_email: str = typer.Option("admin@nasiko.com", envvar="NASIKO_SUPERUSER_EMAIL"),
    provider: str = typer.Option(None, envvar="NASIKO_PROVIDER"),
    region: str = typer.Option(None, envvar="NASIKO_REGION"),
):
    """Deploy backend, web, router, auth + infra to the cluster."""
    from setup.app_setup import deploy
    deploy(
        registry_url=registry_url,
        registry_user=registry_user,
        registry_pass=registry_pass,
        public_user=public_user,
        openai_key=openai_key,
        environment=environment,
        superuser_username=superuser_username,
        superuser_email=superuser_email,
        provider=provider,
        region=region,
    )
