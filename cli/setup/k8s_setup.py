"""
Main 'k8s' command group for setting up Kubernetes clusters.

Terraform Modules and State Management:
- Modules: Located in ~/.nasiko/terraform/ (or NASIKO_TERRAFORM_DIR or --terraform-dir)
- State: Stored in ~/.nasiko/state/<provider>/<cluster>/ (or NASIKO_STATE_DIR or --state-dir)
- Each cluster gets its own working directory with copied modules and state

Remote Backend Support:
- Set NASIKO_TF_BACKEND=s3 and configure bucket via NASIKO_TF_BACKEND_BUCKET
- See config.py for all backend configuration options
"""

import json
import os
import enum
import shutil
import typer
import subprocess
from pathlib import Path
from typing import Optional
from rich.console import Console
from .utils import (
    ensure_terraform,
    ensure_kubectl,
    ensure_nebius_cli,
    setup_terraform_modules,
)
from .config import print_state_info
from .terraform_state import (
    setup_working_directory,
    get_cluster_state_info,
    list_managed_clusters,
    cleanup_cluster_state,
)

# Default cluster name used when none is specified
DEFAULT_CLUSTER_NAME = "nasiko"

app = typer.Typer(
    help="Setup and manage Kubernetes clusters on AWS, DigitalOcean, or Nebius.",
    no_args_is_help=True,
)

console = Console()


class Provider(str, enum.Enum):
    """Enumeration for the supported cloud providers."""

    aws = "aws"
    digitalocean = "digitalocean"
    nebius = "nebius"


def _nebius_cli_query(args: list[str]) -> Optional[str]:
    """Run a `nebius ...` CLI command and return stripped stdout, or None on failure."""
    nebius_bin = shutil.which("nebius") or str(Path.home() / ".nebius" / "bin" / "nebius")
    if not Path(nebius_bin).exists():
        return None
    try:
        result = subprocess.run(
            [nebius_bin, *args],
            capture_output=True,
            text=True,
            check=True,
        )
        return result.stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None


def _resolve_nebius_project_id() -> Optional[str]:
    """Resolve the Nebius project ID from env or the active CLI profile."""
    pid = os.environ.get("NB_PROJECT_ID") or os.environ.get("NEBIUS_PROJECT_ID")
    if pid:
        return pid
    return _nebius_cli_query(["config", "get", "parent-id"])


def _resolve_nebius_subnet_id(project_id: str) -> Optional[str]:
    """Resolve a Nebius VPC subnet ID, defaulting to the first subnet in the project."""
    sid = os.environ.get("NB_SUBNET_ID") or os.environ.get("NEBIUS_SUBNET_ID")
    if sid:
        return sid
    raw = _nebius_cli_query(
        ["vpc", "subnet", "list", "--parent-id", project_id, "--format", "json"]
    )
    if not raw:
        return None
    try:
        data = json.loads(raw)
        items = data.get("items") or []
        if items:
            return items[0].get("metadata", {}).get("id")
    except json.JSONDecodeError:
        pass
    return None


def _resolve_nebius_token() -> Optional[str]:
    """Resolve a Nebius IAM access token from env or `nebius iam get-access-token`."""
    token = os.environ.get("NEBIUS_IAM_TOKEN")
    if token:
        return token
    return _nebius_cli_query(["iam", "get-access-token"])


def _run_command(
    command: list[str],
    cwd: Path,
    env_vars: dict[str, str] = None,
    verbose: bool = False,
):
    """
    Runs a shell command (like terraform) in a specific directory.

    Args:
        command: Command and arguments to execute
        cwd: Working directory
        env_vars: Environment variables to set
        verbose: If True, shows all output. If False, shows only important lines.
    """
    console.print(f"[dim]Running: {' '.join(command)}[/]")

    # Combine with os.environ so existing vars (like AWS_PROFILE) are kept
    process_env = os.environ.copy()
    if env_vars:
        process_env.update(env_vars)

    # Keywords that indicate important output lines
    important_keywords = [
        "error",
        "failed",
        "warning",
        "creating",
        "created",
        "destroying",
        "destroyed",
        "complete",
        "apply complete",
        "plan:",
        "changes",
        "will be created",
        "will be destroyed",
        "will be updated",
    ]

    try:
        # Use Popen to stream output in real-time
        with subprocess.Popen(
            command,
            cwd=cwd,
            env=process_env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,  # Redirect stderr to stdout
            text=True,
            bufsize=1,  # Line-buffered
            encoding="utf-8",
        ) as process:
            if process.stdout:
                for line in iter(process.stdout.readline, ""):
                    # Show all output in verbose mode, or only important lines in quiet mode
                    if verbose or any(
                        keyword in line.lower() for keyword in important_keywords
                    ):
                        print(line, end="")
                    elif (
                        line.strip()
                    ):  # Show a dot for other non-empty lines to indicate progress
                        print(".", end="", flush=True)

            process.wait()  # Wait for the process to complete
            print()  # New line after dots

            if process.returncode != 0:
                raise subprocess.CalledProcessError(process.returncode, command)

    except FileNotFoundError:
        console.print(f"[bold red]Error: '{command[0]}' command not found.[/]")
        console.print("Please install Terraform and ensure it's in your PATH.")
        raise typer.Exit(code=1)
    except subprocess.CalledProcessError as e:
        console.print(f"[bold red]Command failed with exit code {e.returncode}[/]")
        raise typer.Exit(code=e.returncode)


def get_tf_output(work_dir: Path, key: str, env_vars: dict = None) -> Optional[str]:
    """
    Retrieves a specific output variable from the Terraform state.

    Args:
        work_dir: Working directory where terraform state is located
        key: Output key to retrieve
        env_vars: Additional environment variables

    Returns:
        Output value or None if not found
    """
    try:
        process_env = os.environ.copy()
        if env_vars:
            process_env.update(env_vars)

        result = subprocess.run(
            ["terraform", "output", "-raw", key],
            cwd=work_dir,
            env=process_env,
            capture_output=True,
            text=True,
            check=True,
        )
        return result.stdout.strip()
    except subprocess.CalledProcessError:
        return None


def _prepare_tf_vars(
    provider: Provider, cluster_name: str, region: str, node_size: str
) -> dict[str, str]:
    """Prepares a dictionary of environment variables for Terraform."""

    # If called via Python import (setup.py), default args might still be OptionInfo objects.
    if hasattr(node_size, "default"):
        node_size = node_size.default

    tf_vars = {}

    # Set common variables
    if cluster_name:
        tf_vars["TF_VAR_cluster_name"] = cluster_name

    # Set provider-specific variables
    if provider == Provider.aws:
        if region:
            tf_vars["TF_VAR_aws_region"] = region
        if node_size:
            tf_vars["TF_VAR_instance_type"] = node_size

    elif provider == Provider.digitalocean:
        if region:
            tf_vars["TF_VAR_do_region"] = region
        if node_size:
            tf_vars["TF_VAR_node_size"] = node_size

        # Support all token aliases and normalize for Terraform + doctl compatibility.
        do_token = (
            os.environ.get("TF_VAR_do_token")
            or os.environ.get("DIGITALOCEAN_ACCESS_TOKEN")
            or os.environ.get("DO_TOKEN")
        )
        if do_token:
            tf_vars["TF_VAR_do_token"] = do_token
            os.environ.setdefault("DIGITALOCEAN_ACCESS_TOKEN", do_token)
            os.environ.setdefault("DO_TOKEN", do_token)
        else:
            console.print(
                "[bold yellow]DigitalOcean token not found in environment variables "
                "(DIGITALOCEAN_ACCESS_TOKEN, DO_TOKEN, or TF_VAR_do_token).[/]"
            )
            do_token = typer.prompt(
                "Enter DigitalOcean Token (will not be echoed)", hide_input=True
            )
            if not do_token:
                console.print("[bold red]DigitalOcean token is required.[/]")
                raise typer.Exit(1)
            tf_vars["TF_VAR_do_token"] = do_token
            os.environ["DIGITALOCEAN_ACCESS_TOKEN"] = do_token
            os.environ["DO_TOKEN"] = do_token

    elif provider == Provider.nebius:
        # Region is informational (Nebius region is derived from the subnet).
        if region:
            tf_vars["TF_VAR_nb_region"] = region
        # The existing --node-size flag maps to the Nebius compute preset
        # (e.g. "8vcpu-32gb"). Use the module's default when not provided.
        if node_size:
            tf_vars["TF_VAR_node_preset"] = node_size

        # 1. Resolve and export the IAM token (the Nebius Terraform provider
        #    reads NEBIUS_IAM_TOKEN from the environment).
        token = _resolve_nebius_token()
        if not token:
            console.print(
                "[bold red]Nebius IAM token not found.[/]\n"
                "Run [cyan]nebius profile create[/] to set up a profile, then retry. "
                "Alternatively, export [cyan]NEBIUS_IAM_TOKEN[/]."
            )
            raise typer.Exit(1)
        os.environ["NEBIUS_IAM_TOKEN"] = token
        tf_vars["TF_VAR_nb_token"] = token

        # 2. Resolve the project ID (parent_id of the cluster).
        project_id = _resolve_nebius_project_id()
        if not project_id:
            console.print(
                "[bold red]Nebius project ID not found.[/]\n"
                "Set [cyan]NB_PROJECT_ID[/] or run "
                "[cyan]nebius config set parent-id <project-id>[/]."
            )
            raise typer.Exit(1)
        tf_vars["TF_VAR_nb_project_id"] = project_id

        # 3. Resolve the subnet ID (control plane + node group).
        subnet_id = _resolve_nebius_subnet_id(project_id)
        if not subnet_id:
            console.print(
                "[bold red]No Nebius VPC subnet found in the project.[/]\n"
                "Create a subnet first or set [cyan]NB_SUBNET_ID[/]."
            )
            raise typer.Exit(1)
        tf_vars["TF_VAR_nb_subnet_id"] = subnet_id

    return tf_vars


# --- Typer Commands ---


@app.command()
def create(
    provider: Provider = typer.Argument(
        ..., help="Cloud provider to use (aws, digitalocean, or nebius)."
    ),
    cluster_name: str = typer.Option(
        DEFAULT_CLUSTER_NAME, "--name", "-n", help="Name for the Kubernetes cluster."
    ),
    region: str = typer.Option(
        None, help="Region for the cluster (e.g., 'us-east-1', 'nyc3', 'us-central1')."
    ),
    node_size: str = typer.Option(
        None,
        help="Instance type for nodes (e.g., 't3.medium', 's-2vcpu-4gb', '8vcpu-32gb').",
    ),
    auto_approve: bool = typer.Option(
        False, "--yes", "-y", help="Auto-approve Terraform apply."
    ),
    verbose: bool = typer.Option(
        False, "--verbose", "-v", help="Show detailed Terraform output."
    ),
    terraform_dir: str = typer.Option(
        None,
        "--terraform-dir",
        "-t",
        envvar="NASIKO_TERRAFORM_DIR",
        help="Path to Terraform modules directory.",
    ),
    state_dir: str = typer.Option(
        None,
        "--state-dir",
        "-s",
        envvar="NASIKO_STATE_DIR",
        help="Path for storing Terraform state.",
    ),
):
    """
    Create and provision a new Kubernetes cluster.

    State is stored in ~/.nasiko/state/<provider>/<cluster-name>/ by default.
    Set NASIKO_TF_BACKEND=s3 for remote state storage.
    """
    # 1. Ensure required tools are installed
    ensure_terraform()
    ensure_kubectl()
    if provider == Provider.nebius:
        ensure_nebius_cli()

    console.print(f"[green]Starting cluster creation for: [bold]{provider.value}[/][/]")
    console.print(f"[dim]Cluster name: {cluster_name}[/]")

    # 2. Set up working directory with modules and backend config
    try:
        work_dir = setup_working_directory(
            provider=provider.value,
            cluster_name=cluster_name,
            terraform_dir=terraform_dir,
            state_dir=state_dir,
        )
    except FileNotFoundError as e:
        console.print(f"[bold red]{e}[/]")
        raise typer.Exit(code=1)

    # Print state configuration info
    print_state_info(provider.value, cluster_name)

    # 3. Prepare Terraform variables
    env_vars = _prepare_tf_vars(provider, cluster_name, region, node_size)
    run_env = {**os.environ, **env_vars}

    # 4. Terraform Init
    console.print("[cyan]Initializing Terraform...[/]")
    _run_command(
        ["terraform", "init"], cwd=work_dir, env_vars=env_vars, verbose=verbose
    )

    # 5. Terraform Plan
    console.print("[cyan]Planning infrastructure changes...[/]")
    plan_file = "tfplan"
    _run_command(
        ["terraform", "plan", f"-out={plan_file}"],
        cwd=work_dir,
        env_vars=env_vars,
        verbose=verbose,
    )

    # 6. Terraform Apply
    console.print("[cyan]Applying changes...[/]")
    apply_command = ["terraform", "apply"]
    if auto_approve:
        apply_command.append("-auto-approve")
    apply_command.append(plan_file)

    _run_command(apply_command, cwd=work_dir, env_vars=env_vars, verbose=verbose)

    console.rule("[bold green]Cluster provisioning complete![/]")

    # 7. Verify critical addons are installed (AWS only)
    if provider == Provider.aws:
        console.print("[cyan]Verifying EKS addons installation...[/]")
        try:
            cluster_name_val = get_tf_output(work_dir, "cluster_name", env_vars)
            region_val = get_tf_output(work_dir, "region", env_vars)

            if cluster_name_val and region_val:
                addon_check = subprocess.run(
                    [
                        "aws",
                        "eks",
                        "describe-addon",
                        "--cluster-name",
                        cluster_name_val,
                        "--addon-name",
                        "vpc-cni",
                        "--region",
                        region_val,
                    ],
                    capture_output=True,
                    text=True,
                )

                if addon_check.returncode != 0:
                    console.print(
                        "[bold yellow]⚠️  Warning: VPC CNI addon not found. Nodes may fail to join cluster.[/]"
                    )
                    console.print(
                        f"[yellow]Run: aws eks create-addon --cluster-name {cluster_name_val} --addon-name vpc-cni --region {region_val}[/]"
                    )
                else:
                    console.print("[green]✅ VPC CNI addon verified[/]")
        except Exception as e:
            console.print(f"[yellow]⚠️  Could not verify addons: {e}[/]")

    # 8. Save Kubeconfig & Update Env Var
    try:
        console.print("[cyan]Fetching cluster details...[/]")

        actual_cluster_name = (
            get_tf_output(work_dir, "cluster_name", env_vars) or cluster_name
        )
        filename = f"{actual_cluster_name}-kubeconfig.yaml"
        kubeconfig_path = str(work_dir / filename)

        if provider == Provider.aws:
            target_region = region if region else "us-east-1"

            console.print(
                f"[dim]Generating kubeconfig via AWS CLI for {actual_cluster_name} in {target_region}...[/]"
            )
            subprocess.run(
                [
                    "aws",
                    "eks",
                    "update-kubeconfig",
                    "--region",
                    target_region,
                    "--name",
                    actual_cluster_name,
                    "--kubeconfig",
                    kubeconfig_path,
                ],
                check=True,
                capture_output=True,
            )
            # Set proper permissions on kubeconfig (600 = rw-------)
            Path(kubeconfig_path).chmod(0o600)

        elif provider == Provider.digitalocean:
            kube_config = get_tf_output(work_dir, "kube_config", env_vars)
            if kube_config:
                with open(kubeconfig_path, "w") as f:
                    f.write(kube_config)
                # Set proper permissions on kubeconfig (600 = rw-------)
                Path(kubeconfig_path).chmod(0o600)

        elif provider == Provider.nebius:
            cluster_id = get_tf_output(work_dir, "cluster_id", env_vars)
            public_endpoint = get_tf_output(work_dir, "public_endpoint", env_vars)
            if not cluster_id:
                console.print("[red]❌ Could not read cluster_id from Terraform outputs[/]")
                raise typer.Exit(1)

            console.print(
                f"[dim]Generating kubeconfig via Nebius CLI for {actual_cluster_name}...[/]"
            )
            # `nebius mk8s cluster get-credentials --external` writes a
            # kubeconfig pointing at the public load-balanced API endpoint.
            # KUBECONFIG controls the output file path.
            cred_env = {**os.environ, "KUBECONFIG": kubeconfig_path}
            # Ensure the target file exists so the CLI merges into it cleanly.
            Path(kubeconfig_path).touch(mode=0o600, exist_ok=True)
            subprocess.run(
                [
                    "nebius",
                    "mk8s",
                    "cluster",
                    "get-credentials",
                    "--id",
                    cluster_id,
                    "--external",
                ],
                check=True,
                env=cred_env,
                capture_output=True,
                text=True,
            )
            Path(kubeconfig_path).chmod(0o600)

            if public_endpoint:
                console.print(
                    f"[green]✅ Public Kubernetes API: [bold]{public_endpoint}[/][/]"
                )

        os.environ["KUBECONFIG"] = kubeconfig_path

        console.print(f"[green]✅ Kubeconfig saved: [bold]{kubeconfig_path}[/][/]")
        console.print(f"[green]✅ Active KUBECONFIG: [bold]{kubeconfig_path}[/][/]")
        console.print(
            f"[dim]Connect: [cyan]export KUBECONFIG={kubeconfig_path} && kubectl get nodes[/][/]"
        )

    except subprocess.CalledProcessError as e:
        console.print(f"[red]❌ Failed to configure kubeconfig: {e}[/]")
        if e.stderr:
            console.print(f"[dim]{e.stderr}[/]")

    # 9. Configure default storage class for AWS
    if provider == Provider.aws:
        console.print("[cyan]Configuring default storage class...[/]")
        try:
            subprocess.run(
                [
                    "kubectl",
                    "patch",
                    "storageclass",
                    "gp2",
                    "-p",
                    '{"metadata": {"annotations":{"storageclass.kubernetes.io/is-default-class":"true"}}}',
                ],
                check=True,
                capture_output=True,
                text=True,
            )
            console.print("[green]✅ Set gp2 as default storage class[/]")
        except subprocess.CalledProcessError as e:
            console.print(f"[yellow]⚠️  Could not set default storage class: {e}[/]")
            console.print(
                "[yellow]   This may cause PVC provisioning issues. You can set it manually:[/]"
            )
            console.print(
                '[cyan]   kubectl patch storageclass gp2 -p \'{"metadata": {"annotations":{"storageclass.kubernetes.io/is-default-class":"true"}}}\'[/]'
            )


@app.command()
def destroy(
    provider: Provider = typer.Argument(
        ..., help="Cloud provider to destroy (aws, digitalocean, or nebius)."
    ),
    cluster_name: str = typer.Option(
        DEFAULT_CLUSTER_NAME, "--name", "-n", help="Name of the cluster to destroy."
    ),
    auto_approve: bool = typer.Option(
        False, "--yes", "-y", help="Auto-approve Terraform destroy."
    ),
    verbose: bool = typer.Option(
        False, "--verbose", "-v", help="Show detailed Terraform output."
    ),
    terraform_dir: str = typer.Option(
        None,
        "--terraform-dir",
        "-t",
        envvar="NASIKO_TERRAFORM_DIR",
        help="Path to Terraform modules directory.",
    ),
    state_dir: str = typer.Option(
        None,
        "--state-dir",
        "-s",
        envvar="NASIKO_STATE_DIR",
        help="Path for storing Terraform state.",
    ),
    cleanup_state: bool = typer.Option(
        False, "--cleanup", help="Remove local state directory after destroy."
    ),
):
    """
    Destroy the Kubernetes cluster and all related resources.

    Use --name to specify which cluster to destroy (default: nasiko).
    """
    ensure_terraform()
    ensure_kubectl()
    if provider == Provider.nebius:
        ensure_nebius_cli()

    console.print(
        f"[red]Starting cluster destruction for: [bold]{provider.value}/{cluster_name}[/][/]"
    )

    # Check if state exists for this cluster
    state_info = get_cluster_state_info(provider.value, cluster_name, state_dir)
    if not state_info["exists"] and not state_info["has_modules"]:
        console.print(
            f"[yellow]No state found for cluster '{cluster_name}' ({provider.value})[/]"
        )
        console.print("[yellow]Use 'nasiko setup k8s list' to see managed clusters[/]")
        raise typer.Exit(code=1)

    work_dir = state_info["work_dir"]
    print_state_info(provider.value, cluster_name)

    env_vars = _prepare_tf_vars(provider, cluster_name, None, None)

    # 1. Initialize Terraform
    console.print("[cyan]Initializing Terraform...[/]")
    _run_command(
        ["terraform", "init"], cwd=work_dir, env_vars=env_vars, verbose=verbose
    )

    # 2. Run destroy
    console.print("[cyan]Destroying infrastructure...[/]")
    destroy_command = ["terraform", "destroy"]
    if auto_approve:
        destroy_command.append("-auto-approve")

    _run_command(destroy_command, cwd=work_dir, env_vars=env_vars, verbose=verbose)

    console.rule("[bold green]Cluster destruction complete.[/]")

    # 3. Optionally clean up local state
    if cleanup_state:
        cleanup_cluster_state(provider.value, cluster_name, state_dir)


@app.command()
def output(
    provider: Provider = typer.Argument(..., help="Cloud provider to get outputs for."),
    cluster_name: str = typer.Option(
        DEFAULT_CLUSTER_NAME, "--name", "-n", help="Name of the cluster."
    ),
    terraform_dir: str = typer.Option(
        None,
        "--terraform-dir",
        "-t",
        envvar="NASIKO_TERRAFORM_DIR",
        help="Path to Terraform modules directory.",
    ),
    state_dir: str = typer.Option(
        None,
        "--state-dir",
        "-s",
        envvar="NASIKO_STATE_DIR",
        help="Path for storing Terraform state.",
    ),
):
    """
    Display the Terraform outputs for an existing cluster.
    """
    ensure_terraform()

    console.print(
        f"[blue]Fetching outputs for: [bold]{provider.value}/{cluster_name}[/][/]"
    )

    state_info = get_cluster_state_info(provider.value, cluster_name, state_dir)
    if not state_info["exists"] and not state_info["has_modules"]:
        console.print(
            f"[yellow]No state found for cluster '{cluster_name}' ({provider.value})[/]"
        )
        raise typer.Exit(code=1)

    work_dir = state_info["work_dir"]
    env_vars = _prepare_tf_vars(provider, cluster_name, None, None)

    _run_command(["terraform", "output"], cwd=work_dir, env_vars=env_vars)


@app.command("list")
def list_clusters(
    state_dir: str = typer.Option(
        None,
        "--state-dir",
        "-s",
        envvar="NASIKO_STATE_DIR",
        help="Path for storing Terraform state.",
    ),
):
    """
    List all clusters managed by Nasiko CLI.
    """
    clusters = list_managed_clusters(state_dir)

    if not clusters:
        console.print("[yellow]No managed clusters found.[/]")
        console.print(
            "[dim]Create a cluster with: nasiko setup k8s create <provider>[/]"
        )
        return

    console.print("[bold cyan]Managed Clusters:[/]\n")

    for cluster in clusters:
        provider = cluster["provider"]
        name = cluster["cluster_name"]
        work_dir = cluster["work_dir"]

        # Check for state file
        state_file = work_dir / "terraform.tfstate"
        has_state = state_file.exists()

        status = "[green]✓ Active[/]" if has_state else "[yellow]⚠ No state[/]"
        console.print(f"  [cyan]{provider}[/]/[bold]{name}[/]  {status}")
        console.print(f"    [dim]State: {work_dir}[/]")

    console.print()


@app.command()
def state_info(
    provider: Provider = typer.Argument(..., help="Cloud provider."),
    cluster_name: str = typer.Option(
        DEFAULT_CLUSTER_NAME, "--name", "-n", help="Name of the cluster."
    ),
    state_dir: str = typer.Option(
        None,
        "--state-dir",
        "-s",
        envvar="NASIKO_STATE_DIR",
        help="Path for storing Terraform state.",
    ),
):
    """
    Show detailed state information for a cluster.
    """
    state_info_data = get_cluster_state_info(provider.value, cluster_name, state_dir)

    console.print(f"\n[bold cyan]State Info: {provider.value}/{cluster_name}[/]\n")
    console.print(f"  Working Directory: [cyan]{state_info_data['work_dir']}[/]")
    console.print(f"  Backend Type: [cyan]{state_info_data['backend_type']}[/]")
    console.print(
        f"  Has Modules: {'[green]Yes[/]' if state_info_data['has_modules'] else '[yellow]No[/]'}"
    )
    console.print(
        f"  State Exists: {'[green]Yes[/]' if state_info_data['exists'] else '[yellow]No[/]'}"
    )

    if state_info_data["state_file"]:
        console.print(f"  State File: [cyan]{state_info_data['state_file']}[/]")

    print_state_info(provider.value, cluster_name)


@app.command("init-modules")
def init_modules(
    source: str = typer.Option(
        None, "--source", "-s", help="Source directory containing terraform modules."
    ),
    force: bool = typer.Option(
        False, "--force", "-f", help="Overwrite existing modules."
    ),
):
    """
    Initialize Terraform modules in ~/.nasiko/terraform/.

    This command copies Terraform modules from a source location to the
    Nasiko home directory, making them available for cluster management.

    If no source is provided, the CLI will look for modules in the project
    directory structure (for development) or prompt for a path.

    Example:
        nasiko setup k8s init-modules --source /path/to/terraform
    """

    console.print("[cyan]Initializing Terraform modules...[/]\n")

    modules_dir = setup_terraform_modules(source=source, force=force)

    # Verify setup
    aws_exists = (modules_dir / "aws" / "main.tf").exists()
    do_exists = (modules_dir / "digitalocean" / "doks.tf").exists() or (
        modules_dir / "digitalocean" / "main.tf"
    ).exists()
    nebius_exists = (modules_dir / "nebius" / "mk8s.tf").exists()

    console.print("\n[bold cyan]Module Status:[/]")
    console.print(
        f"  AWS: {'[green]✓ Available[/]' if aws_exists else '[red]✗ Not found[/]'}"
    )
    console.print(
        f"  DigitalOcean: {'[green]✓ Available[/]' if do_exists else '[red]✗ Not found[/]'}"
    )
    console.print(
        f"  Nebius: {'[green]✓ Available[/]' if nebius_exists else '[red]✗ Not found[/]'}"
    )
    console.print(f"\n  Location: [cyan]{modules_dir}[/]")

    if not aws_exists or not do_exists or not nebius_exists:
        console.print("\n[yellow]Some modules are missing. Provide the source path:[/]")
        console.print(
            "[dim]  nasiko setup k8s init-modules --source /path/to/terraform[/]"
        )


if __name__ == "__main__":
    app()
