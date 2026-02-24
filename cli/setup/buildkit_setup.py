import sys
import json
import base64
import typer
import yaml
import os
from pathlib import Path
from kubernetes import config, client, utils
from rich.console import Console
from rich.progress import Progress, SpinnerColumn, TextColumn
try:
    from importlib.resources import files
except ImportError:
    # Fallback for Python < 3.9
    from importlib_resources import files

app = typer.Typer(help="Deploy Rootless Buildkit on Kubernetes using YAML manifests")
console = Console()

def get_manifests_dir():
    """Get the path to BuildKit manifests directory."""
    # Try importlib.resources first (works when installed as package)
    try:
        k8s_pkg = files('k8s')
        manifests_path = k8s_pkg / 'charts' / 'nasiko-platform' / 'templates' / 'infrastructure' / 'buildkit'
        # Convert to Path
        manifests_dir = Path(str(manifests_path))
        if manifests_dir.exists():
            return manifests_dir
    except Exception:
        pass
    
    # Fallback: relative path (development mode)
    script_dir = Path(__file__).parent
    manifests_dir = script_dir.parent / "k8s" / "charts" / "nasiko-platform" / "templates" / "infrastructure" / "buildkit"

    if not manifests_dir.exists():
        console.print(f"[red]❌ Manifests directory not found: {manifests_dir}[/]")
        sys.exit(1)

    return manifests_dir

def load_yaml_manifest(file_path):
    """Load and parse a YAML manifest file."""
    try:
        with open(file_path, 'r') as f:
            return yaml.safe_load(f)
    except Exception as e:
        console.print(f"[red]❌ Error loading {file_path}: {e}[/]")
        sys.exit(1)

def apply_manifest(k8s_client, manifest, desc):
    """Helper to apply a K8s object."""
    try:
        utils.create_from_dict(k8s_client, manifest)
        console.print(f"[green]✅ Created {desc}[/]")
    except utils.FailToCreateError as e:
        if hasattr(e, "api_exceptions") and any(x.status == 409 for x in e.api_exceptions):
             console.print(f"[yellow]ℹ️  {desc} already exists, skipping...[/]")
        else:
            console.print(f"[red]❌ Error creating {desc}: {e}[/]")
            sys.exit(1)
    except Exception as e:
        console.print(f"[red]❌ Unexpected error: {e}[/]")
        sys.exit(1)

def create_registry_secret(manifests_dir, k8s_client, registry, username, password):
    """Creates the Docker registry secret using the template manifest."""

    # Extract the registry domain
    registry_domain = registry.split('/')[0]

    # Create the Auth String (user:pass)
    auth_str = f"{username}:{password}"
    auth_b64 = base64.b64encode(auth_str.encode("utf-8")).decode("utf-8")

    # Load secret template
    secret_file = manifests_dir / "regcred-secret.yaml"
    secret_manifest = load_yaml_manifest(secret_file)

    # Replace placeholders in the dockerconfigjson
    docker_config_json = secret_manifest["stringData"][".dockerconfigjson"]
    docker_config_json = docker_config_json.replace("REGISTRY_DOMAIN_PLACEHOLDER", registry_domain)
    docker_config_json = docker_config_json.replace("AUTH_B64_PLACEHOLDER", auth_b64)

    # Parse and update the JSON properly
    docker_config = json.loads(docker_config_json)
    secret_manifest["stringData"][".dockerconfigjson"] = json.dumps(docker_config)

    apply_manifest(k8s_client, secret_manifest, "Registry Secret 'regcred'")

def update_deployment_for_auth_method(manifests_dir, has_credentials, iam_role_arn):
    """Update deployment manifest based on authentication method."""
    deployment_file = manifests_dir / "deployment.yaml"
    deployment_manifest = load_yaml_manifest(deployment_file)

    if iam_role_arn:
        # Use IAM role - update service account
        deployment_manifest["spec"]["template"]["spec"]["serviceAccountName"] = "buildkit-sa"

        # Remove docker config mount (using IAM instead)
        containers = deployment_manifest["spec"]["template"]["spec"]["containers"]
        if containers:
            # Remove docker config mount from volume mounts
            volume_mounts = containers[0].get("volumeMounts", [])
            containers[0]["volumeMounts"] = [
                vm for vm in volume_mounts
                if not (isinstance(vm, dict) and vm.get("name") == "docker-config")
            ]

            # Remove placeholder comment
            volume_mounts_clean = []
            for vm in containers[0]["volumeMounts"]:
                if isinstance(vm, str) and "DOCKER_CONFIG_MOUNT_PLACEHOLDER" in vm:
                    continue
                volume_mounts_clean.append(vm)
            containers[0]["volumeMounts"] = volume_mounts_clean

        # Remove docker config volume
        volumes = deployment_manifest["spec"]["template"]["spec"].get("volumes", [])
        deployment_manifest["spec"]["template"]["spec"]["volumes"] = [
            v for v in volumes
            if not (isinstance(v, dict) and v.get("name") == "docker-config")
            and not (isinstance(v, str) and "DOCKER_CONFIG_VOLUME_PLACEHOLDER" in v)
        ]

    elif has_credentials:
        # Use username/password - add docker config mount
        containers = deployment_manifest["spec"]["template"]["spec"]["containers"]
        if containers:
            # Replace mount placeholder with actual mount
            volume_mounts = containers[0].get("volumeMounts", [])
            volume_mounts_updated = []
            for vm in volume_mounts:
                if isinstance(vm, str) and "DOCKER_CONFIG_MOUNT_PLACEHOLDER" in vm:
                    volume_mounts_updated.append({
                        "name": "docker-config",
                        "mountPath": "/home/user/.docker/config.json",
                        "subPath": ".dockerconfigjson"
                    })
                else:
                    volume_mounts_updated.append(vm)
            containers[0]["volumeMounts"] = volume_mounts_updated

        # Replace volume placeholder with actual volume
        volumes = deployment_manifest["spec"]["template"]["spec"].get("volumes", [])
        volumes_updated = []
        for v in volumes:
            if isinstance(v, str) and "DOCKER_CONFIG_VOLUME_PLACEHOLDER" in v:
                volumes_updated.append({
                    "name": "docker-config",
                    "secret": {
                        "secretName": "regcred",
                        "items": [{
                            "key": ".dockerconfigjson",
                            "path": ".dockerconfigjson"
                        }]
                    }
                })
            else:
                volumes_updated.append(v)
        deployment_manifest["spec"]["template"]["spec"]["volumes"] = volumes_updated

    return deployment_manifest

def update_serviceaccount_for_iam(manifests_dir, iam_role_arn):
    """Update service account with IAM role annotation if provided."""
    sa_file = manifests_dir / "serviceaccount.yaml"
    sa_manifest = load_yaml_manifest(sa_file)

    if iam_role_arn:
        if "annotations" not in sa_manifest["metadata"]:
            sa_manifest["metadata"]["annotations"] = {}
        sa_manifest["metadata"]["annotations"]["eks.amazonaws.com/role-arn"] = iam_role_arn

    return sa_manifest

@app.command()
def deploy(
    registry: str = typer.Option(..., help="Registry URL (e.g., https://reg.example.com or ECR/DO registry)"),
    username: str = typer.Option(None, help="Registry Username (not needed for AWS ECR with IRSA)"),
    password: str = typer.Option(None, help="Registry Password (not needed for AWS ECR with IRSA)"),
    iam_role_arn: str = typer.Option(None, help="AWS IAM Role ARN for IRSA")
):
    """Deploy Rootless Buildkit to the Cluster using YAML manifests.

    For cloud registries with IAM roles (AWS ECR), use --iam-role-arn and skip username/password.
    For Harbor or other registries, provide --username and --password.
    """

    # Remove 'https://' if user included it
    clean_registry = registry.replace("https://", "").replace("http://", "")

    # Validate: Either provide credentials OR IAM role, not neither
    has_credentials = username and password
    has_iam_role = iam_role_arn is not None

    if not has_credentials and not has_iam_role:
        console.print("[bold red]Error: Must provide either (--username and --password) OR --iam-role-arn[/]")
        console.print("\n[yellow]Examples:[/]")
        console.print("  # For Harbor or DockerHub:")
        console.print("  [cyan]--registry harbor.example.com --username admin --password secret[/]")
        console.print("\n  # For AWS ECR with IRSA:")
        console.print("  [cyan]--registry 123456.dkr.ecr.us-east-1.amazonaws.com --iam-role-arn arn:aws:iam::123456:role/buildkit-role[/]")
        sys.exit(1)

    # Get manifests directory
    manifests_dir = get_manifests_dir()

    # Connect to K8s
    try:
        kube_config_path = os.environ.get("KUBECONFIG")

        if kube_config_path:
            console.print(f"[dim]Loading kubeconfig from: {kube_config_path}[/]")
            config.load_kube_config(config_file=kube_config_path)
        else:
            config.load_kube_config()

        k8s_client = client.ApiClient()
        console.print("[bold green]Connected to Kubernetes[/]")

    except Exception as e:
        console.print("[bold red]❌ Failed to load kubeconfig![/]")
        console.print(f"[red]Error Details: {e}[/]")
        if kube_config_path and not os.path.exists(kube_config_path):
             console.print(f"[yellow]⚠️  File not found at: {kube_config_path}[/]")
        sys.exit(1)

    with Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        console=console
    ) as progress:

        task = progress.add_task("Deploying Buildkit...", total=6)

        # 1. Create Namespace
        namespace_manifest = load_yaml_manifest(manifests_dir / "namespace.yaml")
        apply_manifest(k8s_client, namespace_manifest, "Namespace 'buildkit'")
        progress.advance(task)

        # 2. Create ServiceAccount (with IAM role if specified)
        sa_manifest = update_serviceaccount_for_iam(manifests_dir, iam_role_arn)
        apply_manifest(k8s_client, sa_manifest, "ServiceAccount 'buildkit-sa'")
        progress.advance(task)

        # 3. Create Registry Secret (only if credentials provided)
        if has_credentials:
            create_registry_secret(manifests_dir, k8s_client, clean_registry, username, password)
            console.print("[dim]Using username/password authentication[/]")
        elif has_iam_role:
            console.print(f"[dim]Using IAM role authentication: {iam_role_arn}[/]")
        progress.advance(task)

        # 4. Create PVC
        pvc_manifest = load_yaml_manifest(manifests_dir / "pvc.yaml")
        apply_manifest(k8s_client, pvc_manifest, "PVC 'buildkit-cache'")
        progress.advance(task)

        # 5. Create Service
        service_manifest = load_yaml_manifest(manifests_dir / "service.yaml")
        apply_manifest(k8s_client, service_manifest, "Service 'buildkitd'")
        progress.advance(task)

        # 6. Create Deployment (updated for auth method)
        deployment_manifest = update_deployment_for_auth_method(manifests_dir, has_credentials, iam_role_arn)
        apply_manifest(k8s_client, deployment_manifest, "Deployment 'buildkitd'")
        progress.advance(task)

    console.print("\n[bold cyan]Buildkit Deployed Successfully![/]")

    auth_method = "IAM Role (IRSA)" if has_iam_role else "Username/Password"

    console.print(f"""
    [bold]Connection Info:[/bold]
    • Address: [green]tcp://buildkitd.buildkit.svc.cluster.local:1234[/]
    • Registry: [green]{clean_registry}[/]
    • Auth Method: [green]{auth_method}[/]

    [bold]How to use in your Nasiko Backend:[/bold]
    1. Set env var: [cyan]BUILDKIT_HOST=tcp://buildkitd.buildkit.svc.cluster.local:1234[/]
    2. Ensure your job logic sets the image name to: [cyan]{clean_registry}/<your-project>/<image>:<tag>[/]
    """)

if __name__ == "__main__":
    app()