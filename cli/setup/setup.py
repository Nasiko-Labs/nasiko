import typer
import time
import enum
import subprocess
import json
import os
from rich.console import Console
from .utils import get_service_external_ip, ensure_kubectl, setup_terraform_modules
from . import config as config_module
# Import sub-modules
from . import k8s_setup
from . import harbor_setup
from . import buildkit_setup as buildkit_setup
from . import app_setup
from . import container_registry_setup

app = typer.Typer(help="Setup Nasiko cluster components (registry, k8s, core apps).")
console = Console()

# Add sub-commands so they can still be run individually
app.add_typer(k8s_setup.app, name="k8s", help="Manage K8s clusters (AWS/DO)")
app.add_typer(harbor_setup.app, name="harbor", help="Deploy Harbor Registry")
app.add_typer(container_registry_setup.app, name="cloud-reg", help="Setup Cloud Registry") # <--- NEW SUBCOMMAND
app.add_typer(buildkit_setup.app, name="buildkit", help="Deploy Rootless Buildkit")
app.add_typer(app_setup.app, name="core", help="Deploy Nasiko Core Apps (Backend, Web, Router)")

class RegistryType(str, enum.Enum):
    harbor = "harbor"
    cloud = "cloud"

@app.command(name="configure-github-oauth")
def configure_github_oauth(
    config: str = typer.Option(
        None,
        "--config",
        "-c",
        help="Path to .env configuration file (loaded early by the CLI).",
    ),
    kubeconfig: str = typer.Option(
        None,
        envvar="KUBECONFIG",
        help="Path to kubeconfig file (uses KUBECONFIG env var if not specified).",
    ),
    namespace: str = typer.Option("nasiko", help="Kubernetes namespace for nasiko-backend."),
    deployment: str = typer.Option("nasiko-backend", help="Deployment name to patch."),
    container: str = typer.Option(
        None,
        help="Optional container name to patch (defaults to first container if omitted).",
    ),
    restart: bool = typer.Option(
        True,
        "--restart/--no-restart",
        help="Trigger a rollout restart after updating env vars.",
    ),
):
    """
    Patch GitHub OAuth env vars on an existing cluster without re-running bootstrap.

    Reads GITHUB_CLIENT_ID and GITHUB_CLIENT_SECRET from the current process environment
    (typically loaded via `nasiko setup ... -c .hackathon.env`) and updates the
    specified Deployment's pod template env vars.
    """
    from pathlib import Path
    from datetime import datetime, timezone

    # NOTE: Environment files are loaded early in core/cli/main.py before Typer processes args.
    _ = config_module.find_config_file(config)

    client_id = os.getenv("GITHUB_CLIENT_ID", "").strip()
    client_secret = os.getenv("GITHUB_CLIENT_SECRET", "").strip()
    if not client_id or not client_secret:
        console.print("[red]❌ Missing GitHub OAuth env vars.[/]")
        console.print("[yellow]Expected both to be set in the environment:[/]")
        console.print("  [cyan]GITHUB_CLIENT_ID[/]")
        console.print("  [cyan]GITHUB_CLIENT_SECRET[/]")
        console.print("\n[yellow]Tip:[/] run this with your env file, e.g.:")
        console.print("  [cyan]nasiko setup configure-github-oauth -c .hackathon.env[/]")
        raise typer.Exit(1)

    if not kubeconfig:
        console.print("[red]❌ KUBECONFIG is not set.[/]")
        console.print("[yellow]Provide --kubeconfig or export KUBECONFIG.[/]")
        raise typer.Exit(1)

    kubeconfig_path = Path(kubeconfig).resolve()
    if not kubeconfig_path.exists():
        console.print(f"[red]❌ Kubeconfig file not found: {kubeconfig_path}[/]")
        raise typer.Exit(1)

    try:
        from kubernetes import config as k8s_config, client as k8s_client
        k8s_config.load_kube_config(config_file=str(kubeconfig_path))
        apps = k8s_client.AppsV1Api()
    except Exception as e:
        console.print(f"[red]❌ Failed to connect to Kubernetes: {e}[/]")
        raise typer.Exit(1)

    try:
        dep = apps.read_namespaced_deployment(name=deployment, namespace=namespace)
    except Exception as e:
        console.print(f"[red]❌ Failed to read Deployment {namespace}/{deployment}: {e}[/]")
        raise typer.Exit(1)

    spec = dep.spec
    if not spec or not spec.template or not spec.template.spec or not spec.template.spec.containers:
        console.print(f"[red]❌ Deployment {namespace}/{deployment} has no pod template containers[/]")
        raise typer.Exit(1)

    containers = spec.template.spec.containers
    idx = 0
    if container:
        found = False
        for i, c in enumerate(containers):
            if c.name == container:
                idx = i
                found = True
                break
        if not found:
            console.print(f"[red]❌ Container '{container}' not found in {namespace}/{deployment}[/]")
            console.print("[yellow]Available containers:[/]")
            for c in containers:
                console.print(f"  [cyan]{c.name}[/]")
            raise typer.Exit(1)

    env = list(containers[idx].env or [])

    def _set_env(name: str, value: str) -> None:
        for e in env:
            if e.name == name:
                e.value = value
                e.value_from = None
                return
        env.append(k8s_client.V1EnvVar(name=name, value=value))

    _set_env("GITHUB_CLIENT_ID", client_id)
    _set_env("GITHUB_CLIENT_SECRET", client_secret)

    containers[idx].env = env

    # Ensure annotations exist for restart marker
    if restart:
        annotations = spec.template.metadata.annotations or {}
        annotations["kubectl.kubernetes.io/restartedAt"] = datetime.now(timezone.utc).isoformat()
        spec.template.metadata.annotations = annotations

    # Patch only the pod template; avoids clobbering other spec fields.
    patch_body = {
        "spec": {
            "template": {
                "metadata": {
                    "annotations": spec.template.metadata.annotations or {},
                },
                "spec": {
                    "containers": [
                        {
                            "name": containers[idx].name,
                            "env": [{"name": e.name, "value": e.value} for e in env],
                        }
                    ]
                },
            }
        }
    }

    try:
        _ = apps.patch_namespaced_deployment(name=deployment, namespace=namespace, body=patch_body)
    except Exception as e:
        console.print(f"[red]❌ Failed to patch Deployment {namespace}/{deployment}: {e}[/]")
        raise typer.Exit(1)

    console.print(f"[green]✅ Updated GitHub OAuth env vars on {namespace}/{deployment}[/]")
    if restart:
        console.print("[green]✅ Triggered rollout restart[/]")

def _configure_docker_desktop_for_harbor(registry_password: str):
    """Configure Docker Desktop daemon for Harbor registry access"""
    import json
    from pathlib import Path

    # Configuration
    DOCKER_CONFIG_DIR = Path.home() / ".docker"
    DAEMON_JSON_PATH = DOCKER_CONFIG_DIR / "daemon.json"
    HARBOR_NODEPORT = "30500"
    HARBOR_CORE_NODEPORT = "30002"

    console.print("  🔧 Configuring Docker Desktop daemon...")

    # Create .docker directory if it doesn't exist
    DOCKER_CONFIG_DIR.mkdir(exist_ok=True)

    # Backup existing daemon.json if it exists
    if DAEMON_JSON_PATH.exists():
        backup_path = DAEMON_JSON_PATH.with_suffix(f".backup.{int(time.time())}")
        import shutil
        shutil.copy2(DAEMON_JSON_PATH, backup_path)
        console.print(f"    📋 Backed up existing config to {backup_path.name}")

    # Read existing configuration or create new one
    if DAEMON_JSON_PATH.exists():
        with open(DAEMON_JSON_PATH) as f:
            existing_config = json.load(f)
    else:
        existing_config = {}

    # Add insecure registries
    if 'insecure-registries' not in existing_config:
        existing_config['insecure-registries'] = []

    new_registries = [
        f"localhost:{HARBOR_NODEPORT}",
        f"127.0.0.1:{HARBOR_NODEPORT}",
        f"localhost:{HARBOR_CORE_NODEPORT}",
        f"127.0.0.1:{HARBOR_CORE_NODEPORT}"
    ]

    for registry in new_registries:
        if registry not in existing_config['insecure-registries']:
            existing_config['insecure-registries'].append(registry)

    # Write updated configuration
    with open(DAEMON_JSON_PATH, 'w') as f:
        json.dump(existing_config, f, indent=2)

    console.print("    ✅ Docker daemon configuration updated")

    # Restart Docker Desktop
    console.print("    🔄 Restarting Docker Desktop...")
    try:
        # Stop Docker Desktop
        subprocess.run([
            "osascript", "-e", 'tell application "Docker Desktop" to quit'
        ], capture_output=True, timeout=10)
        time.sleep(5)

        # Start Docker Desktop
        subprocess.run(["open", "-a", "Docker Desktop"], check=True)

        # Wait for Docker to be ready
        console.print("    ⏳ Waiting for Docker Desktop to restart...")
        timeout = 180  # Increased from 60 to 180 (6 minutes total)
        for i in range(timeout):
            try:
                subprocess.run(["docker", "version"], capture_output=True, check=True, timeout=5)
                console.print("    ✅ Docker Desktop restarted successfully")
                break
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                time.sleep(2)
        else:
            console.print("    [yellow]⚠️  Timeout waiting for Docker Desktop to start[/]")
            console.print("    [yellow]   Please ensure Docker Desktop is running before deploying agents[/]")
            return

        # Wait for Kubernetes to be ready after Docker restart
        console.print("    ⏳ Waiting for Kubernetes to be ready...")
        k8s_timeout = 120  # 4 minutes for Kubernetes to be ready
        for i in range(k8s_timeout):
            try:
                subprocess.run(["kubectl", "get", "nodes"], capture_output=True, check=True, timeout=10)
                console.print("    ✅ Kubernetes cluster is ready")
                return
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                time.sleep(2)

        console.print("    [yellow]⚠️  Timeout waiting for Docker Desktop to start[/]")
        console.print("    [yellow]   Please ensure Docker Desktop is running before deploying agents[/]")

    except Exception as e:
        console.print(f"    [yellow]⚠️  Could not restart Docker Desktop automatically: {e}[/]")
        console.print("    [yellow]   Please restart Docker Desktop manually[/]")

def _setup_harbor_credentials(registry_user: str, registry_password: str):
    """Setup Kubernetes registry credentials for Harbor access"""
    NAMESPACE = "nasiko-agents"
    HARBOR_NODEPORT = "30500"

    console.print("  🔐 Setting up Kubernetes registry credentials...")

    try:
        # Create namespace
        result = subprocess.run([
            "kubectl", "create", "namespace", NAMESPACE,
            "--dry-run=client", "-o", "yaml"
        ], capture_output=True, text=True, check=True)

        subprocess.run([
            "kubectl", "apply", "-f", "-"
        ], input=result.stdout, text=True, capture_output=True, check=True)

        # Create cluster DNS registry credentials (for BuildKit)
        result = subprocess.run([
            "kubectl", "create", "secret", "docker-registry", "agent-registry-credentials",
            "--docker-server=harbor-registry.harbor.svc.cluster.local:5000",
            f"--docker-username={registry_user}",
            f"--docker-password={registry_password}",
            f"-n", NAMESPACE,
            "--dry-run=client", "-o", "yaml"
        ], capture_output=True, text=True, check=True)

        subprocess.run([
            "kubectl", "apply", "-f", "-"
        ], input=result.stdout, text=True, capture_output=True, check=True)

        # Create NodePort registry credentials (for Docker daemon)
        result = subprocess.run([
            "kubectl", "create", "secret", "docker-registry", "agent-registry-credentials-nodeport",
            f"--docker-server=localhost:{HARBOR_NODEPORT}",
            f"--docker-username={registry_user}",
            f"--docker-password={registry_password}",
            f"-n", NAMESPACE,
            "--dry-run=client", "-o", "yaml"
        ], capture_output=True, text=True, check=True)

        subprocess.run([
            "kubectl", "apply", "-f", "-"
        ], input=result.stdout, text=True, capture_output=True, check=True)

        # Wait for secrets to be fully created before patching
        time.sleep(2)

        # Wait for default service account to be created (K8s creates it automatically but takes time)
        console.print("    ⏳ Waiting for default service account...")
        for i in range(30):  # Wait up to 30 seconds
            try:
                result = subprocess.run([
                    "kubectl", "get", "serviceaccount", "default", "-n", NAMESPACE
                ], capture_output=True, text=True)
                if result.returncode == 0:
                    break
            except Exception:
                pass
            time.sleep(1)
        else:
            # If default SA still doesn't exist, create it
            console.print("    🔧 Creating default service account...")
            subprocess.run([
                "kubectl", "create", "serviceaccount", "default", "-n", NAMESPACE
            ], capture_output=True, check=True)

        # Patch default service account using stdin to avoid shell escaping issues
        patch_json = json.dumps({
            "imagePullSecrets": [
                {"name": "agent-registry-credentials-nodeport"}
            ]
        })

        # Use kubectl patch with --patch-file to avoid shell escaping issues
        subprocess.run([
            "kubectl", "patch", "serviceaccount", "default",
            f"-n", NAMESPACE,
            "--patch", patch_json
        ], capture_output=True, check=True, text=True)

        console.print("    ✅ Kubernetes registry credentials configured")

    except subprocess.CalledProcessError as e:
        console.print(f"    [yellow]⚠️  Error setting up registry credentials: {e}[/]")
        if e.stderr:
            console.print(f"    [yellow]   Error details: {e.stderr}[/]")
        console.print("    [yellow]   You may need to set them up manually[/]")

def _setup_local_port_forwarding():
    """Setup port forwarding for local development access"""
    LOCAL_PORT = "8000"
    GATEWAY_SERVICE = "kong-gateway"
    NAMESPACE = "nasiko"

    console.print(f"  📡 Setting up port forwarding {LOCAL_PORT}:80...")

    # Kill any existing port forwarding to avoid conflicts
    try:
        subprocess.run([
            "pkill", "-f", f"kubectl.*port-forward.*{GATEWAY_SERVICE}"
        ], capture_output=True)
        time.sleep(2)
    except Exception:
        pass  # Ignore if no existing process

    # Wait for the service to be ready
    console.print(f"  ⏳ Waiting for {GATEWAY_SERVICE} service to be ready...")
    max_wait = 30  # 5 minutes
    for i in range(max_wait):
        try:
            result = subprocess.run([
                "kubectl", "get", "svc", GATEWAY_SERVICE, "-n", NAMESPACE
            ], capture_output=True, text=True, check=True)

            # Check if service exists
            if GATEWAY_SERVICE in result.stdout:
                console.print(f"    ✅ Service {GATEWAY_SERVICE} is ready")
                break
        except subprocess.CalledProcessError:
            pass

        if i % 6 == 0:  # Print every 30 seconds
            console.print(f"    [dim]Still waiting for service... ({i*5}s elapsed)[/]")
        time.sleep(5)
    else:
        console.print(f"    [yellow]⚠️  Timeout waiting for {GATEWAY_SERVICE} service[/]")
        console.print(f"    [yellow]   You can set up port forwarding manually later:[/]")
        console.print(f"    [cyan]kubectl port-forward -n {NAMESPACE} svc/{GATEWAY_SERVICE} {LOCAL_PORT}:80[/]")
        return

    # Setup port forwarding in background with subprocess for persistence
    try:
        console.print(f"    🚀 Starting persistent port forwarding...")

        # Start port forwarding as a detached background process using Popen
        with open('/tmp/nasiko-port-forward.log', 'w') as logfile:
            subprocess.Popen([
                "kubectl", "port-forward", "-n", NAMESPACE,
                f"svc/{GATEWAY_SERVICE}", f"{LOCAL_PORT}:80"
            ], stdout=logfile, stderr=subprocess.STDOUT,
               start_new_session=True)  # Detach from parent session

        # Give it more time to start
        time.sleep(5)

        # Test if port forwarding is working with faster feedback
        max_retries = 15  # 30 seconds total for faster feedback
        for retry in range(max_retries):
            try:
                import socket
                sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                sock.settimeout(3)
                result = sock.connect_ex(('localhost', int(LOCAL_PORT)))
                sock.close()

                if result == 0:
                    # Port is accessible, declare success immediately
                    console.print(f"    ✅ Port forwarding active on localhost:{LOCAL_PORT}")
                    # Try to test if gateway responds, but don't fail if it doesn't
                    try:
                        import urllib.request
                        urllib.request.urlopen(f"http://localhost:{LOCAL_PORT}/", timeout=5)
                        console.print(f"    ✅ Gateway responding on localhost:{LOCAL_PORT}")
                    except Exception:
                        console.print(f"    ℹ️  Gateway may still be initializing, but port forwarding is active")
                    break
                else:
                    if retry < max_retries - 1:
                        if retry % 3 == 0:  # Every 6 seconds
                            console.print(f"    [dim]Waiting for port forwarding... ({(retry+1)*2}s elapsed)[/]")
                        time.sleep(2)
                        continue
                    else:
                        console.print(f"    [yellow]⚠️  Port forwarding not ready after {max_retries * 2} seconds[/]")
                        console.print(f"    [yellow]   You can start it manually: kubectl port-forward -n {NAMESPACE} svc/{GATEWAY_SERVICE} {LOCAL_PORT}:80[/]")
            except Exception as e:
                if retry < max_retries - 1:
                    time.sleep(2)
                    continue
                else:
                    console.print(f"    [yellow]⚠️  Cannot verify port forwarding: {e}[/]")
                    console.print(f"    [yellow]   Port forwarding may still be starting in background[/]")

    except Exception as e:
        console.print(f"    [yellow]⚠️  Could not start port forwarding: {e}[/]")
        console.print(f"    [yellow]   You can start it manually:[/]")
        console.print(f"    [cyan]kubectl port-forward -n {NAMESPACE} svc/{GATEWAY_SERVICE} {LOCAL_PORT}:80[/]")

@app.command()
def cleanup(
    kubeconfig: str = typer.Option(..., envvar="KUBECONFIG", help="Path to kubeconfig file for the cluster to cleanup."),
    auto_approve: bool = typer.Option(False, "--yes", "-y", help="Auto-approve cleanup without confirmation.")
):
    """
    🧹 CLEANUP NASIKO RESOURCES
    Removes all Nasiko-related namespaces and resources from a Kubernetes cluster.
    This includes:
    - nasiko namespace (Backend, Web, Router, Auth, Infrastructure)
    - nasiko-agents namespace (Agent deployments)
    - buildkit namespace (BuildKit service)
    """
    import os
    from pathlib import Path
    from kubernetes import config, client

    # Ensure kubectl is available (will auto-download if not found)
    ensure_kubectl()

    # Validate kubeconfig path
    kubeconfig_path = Path(kubeconfig).resolve()
    if not kubeconfig_path.exists():
        console.print(f"[red]❌ Kubeconfig file not found: {kubeconfig_path}[/]")
        raise typer.Exit(1)

    # Fix kubeconfig permissions (should be 600 for security)
    import stat
    current_perms = os.stat(kubeconfig_path).st_mode
    if current_perms & (stat.S_IRWXG | stat.S_IRWXO):  # If group or other has any permissions
        try:
            os.chmod(kubeconfig_path, 0o600)  # Set to rw-------
            console.print(f"[dim]Fixed kubeconfig permissions to 600[/]")
        except Exception as e:
            console.print(f"[yellow]⚠️  Could not fix kubeconfig permissions: {e}[/]")

    # Load kubeconfig
    try:
        console.print(f"[dim]Loading kubeconfig from: {kubeconfig_path}[/]")
        config.load_kube_config(config_file=str(kubeconfig_path))
        v1 = client.CoreV1Api()
        console.print("[green]✅ Connected to Kubernetes cluster[/]")

        # Get cluster info
        try:
            cluster_info = v1.list_namespace()
            console.print(f"[cyan]ℹ️  Cluster has {len(cluster_info.items)} namespaces[/]")
        except Exception as e:
            console.print(f"[yellow]⚠️  Warning: Could not list namespaces: {e}[/]")

    except Exception as e:
        console.print(f"[red]❌ Failed to load kubeconfig: {e}[/]")
        raise typer.Exit(1)

    # Confirm cleanup
    if not auto_approve:
        console.print("\n[bold yellow]⚠️  WARNING: This will delete the following resources:[/]")
        console.print("  • All resources in 'nasiko' namespace")
        console.print("  • All resources in 'nasiko-agents' namespace")
        console.print("  • All resources in 'buildkit' namespace")
        console.print("  • Helm releases: redis, mongodb, kong, nasiko-auth")
        console.print("  • All agent deployments and builds\n")

        confirm = typer.confirm("Are you sure you want to proceed?")
        if not confirm:
            console.print("[yellow]Cleanup cancelled.[/]")
            raise typer.Exit(0)

    # Set KUBECONFIG environment variable for subprocess commands
    os.environ["KUBECONFIG"] = str(kubeconfig_path)

    # Step 1: Delete Helm Releases
    console.rule("[bold magenta]Removing Helm Releases[/]")
    helm_releases = [
        ("nasiko-auth", "nasiko"),
        ("kong", "nasiko"),
        ("mongodb", "nasiko"),
        ("redis", "nasiko")
    ]

    for release_name, namespace in helm_releases:
        try:
            result = subprocess.run(
                ["helm", "uninstall", release_name, "-n", namespace],
                capture_output=True,
                text=True,
                timeout=30
            )
            if result.returncode == 0:
                console.print(f"[green]✅ Removed Helm release: {release_name}[/]")
            else:
                console.print(f"[yellow]ℹ️  Helm release '{release_name}' not found or already removed[/]")
        except Exception as e:
            console.print(f"[yellow]⚠️  Error removing Helm release '{release_name}': {e}[/]")

    # Step 2: Delete Namespaces
    console.rule("[bold magenta]Deleting Namespaces[/]")
    namespaces_to_delete = ["nasiko", "nasiko-agents", "buildkit"]

    for ns in namespaces_to_delete:
        try:
            v1.delete_namespace(name=ns)
            console.print(f"[green]✅ Deleted namespace: {ns}[/]")
        except client.exceptions.ApiException as e:
            if e.status == 404:
                console.print(f"[yellow]ℹ️  Namespace '{ns}' not found (already deleted)[/]")
            else:
                console.print(f"[red]❌ Error deleting namespace '{ns}': {e}[/]")
        except Exception as e:
            console.print(f"[red]❌ Unexpected error deleting namespace '{ns}': {e}[/]")

    # Step 3: Wait for namespaces to be fully deleted
    console.rule("[bold magenta]Waiting for Namespace Termination[/]")
    console.print("[yellow]⏳ Waiting for namespaces to fully terminate...[/]")

    max_wait = 120  # Maximum 2 minutes
    wait_interval = 5  # Check every 5 seconds
    elapsed = 0

    while elapsed < max_wait:
        try:
            remaining_namespaces = v1.list_namespace()
            nasiko_namespaces = [ns.metadata.name for ns in remaining_namespaces.items
                               if ns.metadata.name in namespaces_to_delete]

            if not nasiko_namespaces:
                console.print(f"[green]✅ All namespaces fully deleted ({elapsed}s)[/]")
                break
            else:
                console.print(f"[dim]  Still terminating: {', '.join(nasiko_namespaces)} ({elapsed}s elapsed)[/]")
                time.sleep(wait_interval)
                elapsed += wait_interval
        except Exception as e:
            console.print(f"[yellow]⚠️  Error checking namespaces: {e}[/]")
            break

    if elapsed >= max_wait:
        console.print(f"[yellow]⚠️  Timeout after {max_wait}s - some namespaces may still be terminating[/]")
        console.print("[dim]   They will be fully deleted shortly.[/]")

    console.print("\n[bold green]🧹 Cleanup complete![/]")
    console.print("[dim]The cluster is now clean and ready for a fresh Nasiko installation.[/]")

@app.command()
def init_superuser(
    # Cluster Connection
    kubeconfig: str = typer.Option(None, envvar="KUBECONFIG", help="Path to kubeconfig file. Uses KUBECONFIG env var if not specified."),

    # Super User Configuration
    superuser_username: str = typer.Option("admin", envvar="NASIKO_SUPERUSER_USERNAME", help="Super user username."),
    superuser_email: str = typer.Option("admin@nasiko.com", envvar="NASIKO_SUPERUSER_EMAIL", help="Super user email."),

    # Optional provider for credential file naming
    provider: k8s_setup.Provider = typer.Option(None, envvar="NASIKO_PROVIDER", help="Cloud provider (optional, for credential file naming)."),
):
    """
    🔐 INITIALIZE SUPER USER

    Creates a super user in an existing Nasiko cluster and retrieves credentials.
    This is useful when:
    - You need to recreate super user credentials
    - You deployed core apps without bootstrap
    - You want to create additional super users

    Steps:
    1. Connects to the cluster using kubeconfig (from --kubeconfig or KUBECONFIG env var)
    2. Creates/triggers the superuser-init job
    3. Waits for job completion (max 5 minutes)
    4. Retrieves credentials from Kubernetes secret
    5. Displays and saves credentials locally

    Example:
        nasiko setup init-superuser --kubeconfig ~/.kube/config
        # OR using environment variable:
        export KUBECONFIG=~/.kube/config
        nasiko setup init-superuser

    The credentials will be saved to ~/.nasiko/credentials/<context-name>-superuser.json
    """
    from pathlib import Path
    import time
    import json
    import base64
    from kubernetes import config as k8s_config, client

    console.print(f"[bold cyan]🔐 Initializing Super User[/]")

    # Ensure kubectl is available
    ensure_kubectl()

    # Get kubeconfig path (from parameter or environment variable)
    if not kubeconfig:
        kubeconfig = os.environ.get("KUBECONFIG")
        if not kubeconfig:
            console.print("[red]❌ No kubeconfig specified. Set --kubeconfig or KUBECONFIG environment variable.[/]")
            raise typer.Exit(1)

    # Validate kubeconfig path
    kubeconfig_path = Path(kubeconfig).resolve()

    # Check if file exists and is accessible
    try:
        if not kubeconfig_path.exists():
            console.print(f"[red]❌ Kubeconfig file not found: {kubeconfig_path}[/]")
            raise typer.Exit(1)
    except PermissionError:
        console.print(f"[red]❌ Permission denied accessing kubeconfig: {kubeconfig_path}[/]")
        console.print(f"[yellow]   Try fixing permissions with: chmod 600 {kubeconfig_path}[/]")
        raise typer.Exit(1)

    # Fix kubeconfig permissions if needed
    import stat
    try:
        current_perms = os.stat(kubeconfig_path).st_mode
        if current_perms & (stat.S_IRWXG | stat.S_IRWXO):
            try:
                os.chmod(kubeconfig_path, 0o600)
                console.print(f"[dim]Fixed kubeconfig permissions to 600[/]")
            except Exception as e:
                console.print(f"[yellow]⚠️  Could not fix kubeconfig permissions: {e}[/]")
    except PermissionError:
        console.print(f"[yellow]⚠️  Cannot check kubeconfig permissions. Please ensure it's readable.[/]")
        console.print(f"[yellow]   Try: chmod 600 {kubeconfig_path}[/]")

    # Load kubeconfig and connect to cluster
    try:
        console.print(f"[dim]Loading kubeconfig from: {kubeconfig_path}[/]")
        k8s_config.load_kube_config(config_file=str(kubeconfig_path))

        # Get current context name for credential file naming
        contexts, active_context = k8s_config.list_kube_config_contexts(config_file=str(kubeconfig_path))
        cluster_name = active_context['name'] if active_context else "default-cluster"

        v1 = client.CoreV1Api()
        console.print(f"[green]✅ Connected to Kubernetes cluster (context: {cluster_name})[/]")
    except Exception as e:
        console.print(f"[red]❌ Failed to load kubeconfig: {e}[/]")
        raise typer.Exit(1)

    # Set KUBECONFIG environment variable
    os.environ["KUBECONFIG"] = str(kubeconfig_path)

    # Check if nasiko namespace exists
    try:
        v1.read_namespace("nasiko")
        console.print("[green]✅ Found 'nasiko' namespace[/]")
    except Exception:
        console.print("[red]❌ 'nasiko' namespace not found. Please deploy Nasiko core apps first.[/]")
        console.print("[cyan]   Run: nasiko setup core deploy[/]")
        raise typer.Exit(1)

    # Check if superuser credentials already exist
    try:
        existing_secret = v1.read_namespaced_secret(name="superuser-credentials", namespace="nasiko")
        console.print("[yellow]⚠️  Super user credentials already exist in the cluster[/]")

        # Prompt for confirmation
        confirm = typer.confirm("Do you want to reinitialize and overwrite existing credentials?", default=False)
        if not confirm:
            console.print("[cyan]ℹ️  Initialization cancelled. Use 'nasiko setup get-superuser' to retrieve existing credentials.[/]")
            raise typer.Exit(0)

        console.print("[yellow]⚠️  Proceeding to reinitialize super user credentials...[/]")

        # Delete existing secret and job so a new job can run
        try:
            v1.delete_namespaced_secret(name="superuser-credentials", namespace="nasiko")
            console.print("[dim]Deleted existing credentials secret[/]")
        except Exception as e:
            console.print(f"[yellow]⚠️  Could not delete existing secret: {e}[/]")

        # Delete the existing job so it can be recreated and run again
        try:
            batch_v1 = client.BatchV1Api()
            batch_v1.delete_namespaced_job(
                name="superuser-init",
                namespace="nasiko",
                body=client.V1DeleteOptions(propagation_policy='Foreground')
            )
            console.print("[dim]Deleted existing superuser-init job[/]")
            # Wait for job to be fully deleted
            time.sleep(3)
        except client.exceptions.ApiException as e:
            if e.status != 404:  # Ignore if job doesn't exist
                console.print(f"[yellow]⚠️  Could not delete existing job: {e}[/]")
        except Exception as e:
            console.print(f"[yellow]⚠️  Could not delete existing job: {e}[/]")

    except client.exceptions.ApiException as e:
        if e.status == 404:
            console.print("[dim]No existing credentials found[/]")
        else:
            console.print(f"[yellow]⚠️  Could not check for existing credentials: {e}[/]")

    # Deploy/trigger the superuser-init job using app_setup module
    console.rule("[bold magenta]Creating Super User Job[/]")

    # Delete existing user to allow recreation with new credentials
    # The auth service doesn't have a DELETE endpoint, so we delete directly from MongoDB
    try:
        console.print("[dim]Checking for existing user in database...[/]")

        # Get MongoDB credentials
        try:
            mongodb_secret = v1.read_namespaced_secret(name="mongodb", namespace="nasiko")
            import base64
            mongodb_password = base64.b64decode(mongodb_secret.data.get('mongodb-root-password', '')).decode('utf-8')

            # Get MongoDB pod
            pods = v1.list_namespaced_pod(
                namespace="nasiko",
                label_selector="app.kubernetes.io/name=mongodb"
            )

            if not pods.items:
                console.print("[yellow]⚠️  MongoDB pod not found, skipping user deletion[/]")
            else:
                mongodb_pod = pods.items[0].metadata.name

                # Delete user from MongoDB using kubectl exec
                delete_cmd = [
                    "kubectl", "exec", "-n", "nasiko", mongodb_pod, "--",
                    "mongosh", "-u", "root", "-p", mongodb_password,
                    "--authenticationDatabase", "admin", "nasiko",
                    "--eval", f'db.users.deleteOne({{username: "{superuser_username}"}})'
                ]

                result = subprocess.run(
                    delete_cmd,
                    capture_output=True,
                    text=True,
                    timeout=10
                )

                if "deletedCount: 1" in result.stdout:
                    console.print(f"[dim]Deleted existing user '{superuser_username}' from database[/]")
                    time.sleep(2)  # Wait for deletion to propagate
                elif "deletedCount: 0" in result.stdout:
                    console.print(f"[dim]User '{superuser_username}' does not exist yet (expected for first run)[/]")
                else:
                    console.print(f"[yellow]⚠️  Could not confirm user deletion: {result.stdout[:100]}[/]")

        except Exception as e:
            console.print(f"[yellow]⚠️  Could not delete existing user from database: {e}[/]")
            console.print("[yellow]   Proceeding anyway - job will fail if user exists[/]")
    except Exception as e:
        console.print(f"[yellow]⚠️  Error during user cleanup: {e}[/]")

    # Always delete any existing job first to ensure a fresh run
    try:
        batch_v1_pre = client.BatchV1Api()
        batch_v1_pre.delete_namespaced_job(
            name="superuser-init",
            namespace="nasiko",
            body=client.V1DeleteOptions(propagation_policy='Foreground')
        )
        console.print("[dim]Deleted existing superuser-init job for fresh run[/]")
        # Wait for job to be fully deleted
        time.sleep(3)
    except client.exceptions.ApiException as e:
        if e.status != 404:  # Ignore if job doesn't exist
            console.print(f"[yellow]⚠️  Could not delete existing job: {e}[/]")
    except Exception as e:
        console.print(f"[yellow]⚠️  Could not delete existing job: {e}[/]")

    try:
        # Initialize deployer to use its deploy_superuser_init method
        k8s_client = client.ApiClient()
        registry_config = {"url": "placeholder", "public": "placeholder"}
        from . import app_setup
        deployer = app_setup.NasikoDeployer(
            k8s_client=k8s_client,
            registry_config=registry_config,
            environment="default"
        )

        # Deploy the superuser init job
        deployer.deploy_superuser_init(username=superuser_username, email=superuser_email)

    except Exception as e:
        console.print(f"[red]❌ Failed to create superuser job: {e}[/]")
        console.print(f"[yellow]   You may need to manually create the job[/]")
        raise typer.Exit(1)

    # Wait for super user initialization to complete
    console.rule("[bold magenta]Waiting for Super User Creation[/]")

    try:
        batch_v1 = client.BatchV1Api()
        core_v1 = client.CoreV1Api()

        console.print("[yellow]⏳ Waiting for super user initialization job to complete...[/]")

        # Wait for job to complete (max 5 minutes)
        max_wait = 60  # 5 minutes
        for i in range(max_wait):
            try:
                job = batch_v1.read_namespaced_job_status(name="superuser-init", namespace="nasiko")
                if job.status.succeeded and job.status.succeeded > 0:
                    console.print("[green]✅ Super user initialized successfully![/]")
                    break
                elif job.status.failed and job.status.failed > 0:
                    console.print("[yellow]⚠️  Super user job failed, checking if user already exists...[/]")
                    break
            except Exception:
                pass  # Job not found yet

            if i % 6 == 0:  # Print every 30 seconds
                console.print(f"[dim]  Still waiting... ({i*5}s elapsed)[/]")
            time.sleep(5)
        else:
            console.print("[yellow]⚠️  Timeout waiting for job completion[/]")

        # Retrieve credentials from secret
        time.sleep(5)  # Small buffer for secret creation

        try:
            secret = core_v1.read_namespaced_secret(name="superuser-credentials", namespace="nasiko")

            # Decode credentials
            username = base64.b64decode(secret.data.get('username', '')).decode('utf-8')
            email = base64.b64decode(secret.data.get('email', '')).decode('utf-8')
            user_id = base64.b64decode(secret.data.get('user_id', '')).decode('utf-8')
            access_key = base64.b64decode(secret.data.get('access_key', '')).decode('utf-8')
            access_secret = base64.b64decode(secret.data.get('access_secret', '')).decode('utf-8')

            # Display credentials prominently
            console.print(f"\n[bold green]{'='*60}[/]")
            console.print(f"[bold green]  SUPER USER CREDENTIALS (SAVE THESE!)[/]")
            console.print(f"[bold green]{'='*60}[/]")
            console.print(f"  [bold]Username:[/]      [cyan]{username}[/]")
            console.print(f"  [bold]Email:[/]         [cyan]{email}[/]")
            console.print(f"  [bold]User ID:[/]       [dim]{user_id}[/]")
            console.print(f"  [bold]Access Key:[/]    [yellow]{access_key}[/]")
            console.print(f"  [bold]Access Secret:[/] [yellow]{access_secret}[/]")
            console.print(f"[bold green]{'='*60}[/]")
            console.print(f"\n[bold red]⚠️  Store the Access Secret securely - it won't be shown again![/]\n")

            # Save to local file
            creds_file = config_module.get_cluster_credentials_file(
                cluster_name=cluster_name,
                provider=provider.value if provider else None
            )
            with open(creds_file, 'w') as f:
                json.dump({
                    "username": username,
                    "email": email,
                    "user_id": user_id,
                    "access_key": access_key,
                    "access_secret": access_secret,
                    "created_at": time.strftime("%Y-%m-%d %H:%M:%S"),
                    "cluster_context": cluster_name
                }, f, indent=2)
            creds_file.chmod(0o600)
            console.print(f"[green]✅ Credentials saved to: {creds_file}[/]\n")

        except Exception as e:
            console.print(f"[yellow]⚠️  Could not retrieve credentials from secret: {e}[/]")
            console.print(f"[yellow]   You can retrieve them later using:[/]")
            console.print(f"[cyan]   kubectl get secret superuser-credentials -n nasiko -o json | jq -r '.data | map_values(@base64d)'[/]\n")
            raise typer.Exit(1)

    except Exception as e:
        console.print(f"[red]❌ Error during super user initialization: {e}[/]")
        console.print(f"[yellow]   Check job logs with:[/]")
        console.print(f"[cyan]   kubectl logs -n nasiko job/superuser-init[/]\n")
        raise typer.Exit(1)

    console.print("[bold green]✅ Super user initialization complete![/]")

@app.command()
def get_superuser(
    # Cluster Connection
    kubeconfig: str = typer.Option(None, envvar="KUBECONFIG", help="Path to kubeconfig file. Uses KUBECONFIG env var if not specified."),

    # Optional provider for credential file naming
    provider: k8s_setup.Provider = typer.Option(None, envvar="NASIKO_PROVIDER", help="Cloud provider (optional, for credential file naming)."),

    # Output options
    save_to_file: bool = typer.Option(True, "--save/--no-save", help="Save credentials to local file."),
):
    """
    🔍 FETCH SUPER USER CREDENTIALS

    Retrieves existing super user credentials from the cluster without creating or modifying anything.
    This is a read-only operation that is completely safe to run on existing clusters.

    Use this when:
    - You lost your local credential file
    - You need to retrieve credentials created by someone else
    - You want to verify current super user credentials

    Steps:
    1. Connects to the cluster using kubeconfig
    2. Reads the 'superuser-credentials' secret from 'nasiko' namespace
    3. Decodes and displays the credentials
    4. Optionally saves to local file

    Example:
        nasiko setup get-superuser --kubeconfig ~/.kube/config
        # OR using environment variable:
        export KUBECONFIG=~/.kube/config
        nasiko setup get-superuser

    This is a READ-ONLY operation - it will not modify anything in your cluster.
    """
    from pathlib import Path
    import time
    import json
    import base64
    from kubernetes import config as k8s_config, client

    console.print(f"[bold cyan]🔍 Fetching Super User Credentials[/]")

    # Ensure kubectl is available
    ensure_kubectl()

    # Get kubeconfig path (from parameter or environment variable)
    if not kubeconfig:
        kubeconfig = os.environ.get("KUBECONFIG")
        if not kubeconfig:
            console.print("[red]❌ No kubeconfig specified. Set --kubeconfig or KUBECONFIG environment variable.[/]")
            raise typer.Exit(1)

    # Validate kubeconfig path
    kubeconfig_path = Path(kubeconfig).resolve()

    # Check if file exists and is accessible
    try:
        if not kubeconfig_path.exists():
            console.print(f"[red]❌ Kubeconfig file not found: {kubeconfig_path}[/]")
            raise typer.Exit(1)
    except PermissionError:
        console.print(f"[red]❌ Permission denied accessing kubeconfig: {kubeconfig_path}[/]")
        console.print(f"[yellow]   Try fixing permissions with: chmod 600 {kubeconfig_path}[/]")
        raise typer.Exit(1)

    # Fix kubeconfig permissions if needed
    import stat
    try:
        current_perms = os.stat(kubeconfig_path).st_mode
        if current_perms & (stat.S_IRWXG | stat.S_IRWXO):
            try:
                os.chmod(kubeconfig_path, 0o600)
                console.print(f"[dim]Fixed kubeconfig permissions to 600[/]")
            except Exception as e:
                console.print(f"[yellow]⚠️  Could not fix kubeconfig permissions: {e}[/]")
    except PermissionError:
        console.print(f"[yellow]⚠️  Cannot check kubeconfig permissions. Please ensure it's readable.[/]")
        console.print(f"[yellow]   Try: chmod 600 {kubeconfig_path}[/]")

    # Load kubeconfig and connect to cluster
    try:
        console.print(f"[dim]Loading kubeconfig from: {kubeconfig_path}[/]")
        k8s_config.load_kube_config(config_file=str(kubeconfig_path))

        # Get current context name for credential file naming
        contexts, active_context = k8s_config.list_kube_config_contexts(config_file=str(kubeconfig_path))
        cluster_name = active_context['name'] if active_context else "default-cluster"

        v1 = client.CoreV1Api()
        console.print(f"[green]✅ Connected to Kubernetes cluster (context: {cluster_name})[/]")
    except Exception as e:
        console.print(f"[red]❌ Failed to load kubeconfig: {e}[/]")
        raise typer.Exit(1)

    # Check if nasiko namespace exists
    try:
        v1.read_namespace("nasiko")
        console.print("[green]✅ Found 'nasiko' namespace[/]")
    except Exception:
        console.print("[red]❌ 'nasiko' namespace not found. Please deploy Nasiko core apps first.[/]")
        raise typer.Exit(1)

    # Retrieve credentials from secret
    console.rule("[bold magenta]Retrieving Credentials from Secret[/]")

    try:
        core_v1 = client.CoreV1Api()

        console.print("[yellow]⏳ Reading 'superuser-credentials' secret...[/]")
        secret = core_v1.read_namespaced_secret(name="superuser-credentials", namespace="nasiko")

        # Decode credentials
        username = base64.b64decode(secret.data.get('username', '')).decode('utf-8')
        email = base64.b64decode(secret.data.get('email', '')).decode('utf-8')
        user_id = base64.b64decode(secret.data.get('user_id', '')).decode('utf-8')
        access_key = base64.b64decode(secret.data.get('access_key', '')).decode('utf-8')
        access_secret = base64.b64decode(secret.data.get('access_secret', '')).decode('utf-8')

        # Display credentials prominently
        console.print(f"\n[bold green]{'='*60}[/]")
        console.print(f"[bold green]  SUPER USER CREDENTIALS[/]")
        console.print(f"[bold green]{'='*60}[/]")
        console.print(f"  [bold]Username:[/]      [cyan]{username}[/]")
        console.print(f"  [bold]Email:[/]         [cyan]{email}[/]")
        console.print(f"  [bold]User ID:[/]       [dim]{user_id}[/]")
        console.print(f"  [bold]Access Key:[/]    [yellow]{access_key}[/]")
        console.print(f"  [bold]Access Secret:[/] [yellow]{access_secret}[/]")
        console.print(f"[bold green]{'='*60}[/]")
        console.print(f"\n[bold red]⚠️  Keep the Access Secret secure![/]\n")

        # Save to local file if requested
        if save_to_file:
            creds_file = config_module.get_cluster_credentials_file(
                cluster_name=cluster_name,
                provider=provider.value if provider else None
            )
            with open(creds_file, 'w') as f:
                json.dump({
                    "username": username,
                    "email": email,
                    "user_id": user_id,
                    "access_key": access_key,
                    "access_secret": access_secret,
                    "retrieved_at": time.strftime("%Y-%m-%d %H:%M:%S"),
                    "cluster_context": cluster_name
                }, f, indent=2)
            creds_file.chmod(0o600)
            console.print(f"[green]✅ Credentials saved to: {creds_file}[/]\n")
        else:
            console.print(f"[dim]ℹ️  Credentials not saved to file (use --save to save)[/]\n")

    except client.exceptions.ApiException as e:
        if e.status == 404:
            console.print(f"[red]❌ Secret 'superuser-credentials' not found in 'nasiko' namespace[/]")
            console.print(f"[yellow]   The super user may not have been initialized yet.[/]")
            console.print(f"[cyan]   Run: nasiko setup init-superuser[/]\n")
        else:
            console.print(f"[red]❌ Error reading secret: {e}[/]")
        raise typer.Exit(1)
    except Exception as e:
        console.print(f"[red]❌ Error retrieving credentials: {e}[/]")
        console.print(f"[yellow]   You can try manually:[/]")
        console.print(f"[cyan]   kubectl get secret superuser-credentials -n nasiko -o json | jq -r '.data | map_values(@base64d)'[/]\n")
        raise typer.Exit(1)

    console.print("[bold green]✅ Credentials retrieved successfully![/]")

@app.command()
def bootstrap(
    # Configuration File
    config: str = typer.Option(None, "--config", "-c", help="Path to .env configuration file. Auto-detects .nasiko.env, .nasiko-aws.env, .nasiko-do.env, or .env if not specified."),

    # Cluster Selection Args
    kubeconfig: str = typer.Option(None, envvar="KUBECONFIG", help="Path to existing kubeconfig (skips cluster provisioning if provided)."),

    # Infrastructure Args (only used if kubeconfig is NOT provided)
    provider: k8s_setup.Provider = typer.Option(None, envvar="NASIKO_PROVIDER", help="Cloud provider (aws or digitalocean). Required if --kubeconfig is not provided."),
    cluster_name: str = typer.Option("nasiko-cluster", envvar="NASIKO_CLUSTER_NAME", help="Name for the Kubernetes cluster."),
    region: str = typer.Option(None, envvar="NASIKO_REGION", help="Region (e.g., nyc1, us-east-1). Defaults if omitted."),

    # Terraform configuration (for new cluster provisioning)
    terraform_dir: str = typer.Option(None, "--terraform-dir", "-t", envvar="NASIKO_TERRAFORM_DIR",
                                       help="Path to Terraform modules directory."),
    state_dir: str = typer.Option(None, "--state-dir", envvar="NASIKO_STATE_DIR",
                                   help="Path for storing Terraform state."),

    # Registry Selection
    registry_type: RegistryType = typer.Option(RegistryType.harbor, envvar="NASIKO_CONTAINER_REGISTRY_TYPE", help="Type of registry to use."),

    # Harbor & Domain Args (Required if registry_type is harbor)
    domain: str = typer.Option(None, envvar="NASIKO_DOMAIN", help="Base domain for Harbor (Required for Harbor)."),
    email: str = typer.Option(None, envvar="NASIKO_EMAIL", help="Email for SSL certificates (Required for Harbor)."),
    registry_user: str = typer.Option("admin", envvar="NASIKO_REGISTRY_USER", help="Username for Harbor admin and registry API."),
    registry_pass: str = typer.Option(None, envvar="NASIKO_REGISTRY_PASS", help="Password for Harbor admin and registry API."),

    # Cloud Registry Args (Required if registry_type is cloud)
    cloud_reg_name: str = typer.Option("nasiko-images", envvar="NASIKO_CONTAINER_REGISTRY_NAME", help="Name for ECR Repo or DO Registry."),

    # App Configuration Args
    openai_key: str = typer.Option(None, envvar="OPENAI_API_KEY", help="OpenAI API Key for the Router agent (Optional)."),
    public_registry_user: str = typer.Option(config_module.DEFAULT_PUBLIC_REGISTRY_USER, envvar="NASIKO_PUBLIC_REGISTRY_USER", help="Docker Hub user to pull core apps from."),

    # Super User Args
    superuser_username: str = typer.Option("admin", envvar="NASIKO_SUPERUSER_USERNAME", help="Super user username (default: admin)."),
    superuser_email: str = typer.Option("admin@nasiko.com", envvar="NASIKO_SUPERUSER_EMAIL", help="Super user email (default: admin@nasiko.com)."),

    # Cleanup Args
    clean_existing: bool = typer.Option(True, help="Clean up existing Nasiko resources before deployment.")
):
    """
    🚀 SINGLE COMMAND SETUP

    Supports configuration via:
    - Environment file (.nasiko.env, .nasiko-aws.env, .nasiko-do.env, .env)
    - Environment variables (NASIKO_PROVIDER, NASIKO_REGION, etc.)
    - CLI arguments (highest priority)

    Option 1 (Existing Cluster): Use --kubeconfig to deploy to an existing cluster
    Option 2 (New Cluster): Use --provider to provision a new cluster via Terraform

    Steps:
    1. Provisions K8s Cluster (Terraform) OR Uses Existing Cluster
    2. Cleans up existing Nasiko resources (if --clean-existing)
    3. Deploys/Configures Registry (Harbor OR Cloud Provider)
    4. Deploys Buildkit (connected to Registry)
    5. Deploys Nasiko Core Apps

    Example with config file:
        nasiko setup bootstrap --config .nasiko-aws.env

    Example with environment variables:
        export NASIKO_PROVIDER=digitalocean
        export NASIKO_REGION=nyc3
        export DIGITALOCEAN_ACCESS_TOKEN=dop_v1_...
        nasiko setup bootstrap
    """
    from pathlib import Path

    # --- Configuration Info ---
    # Note: Environment files are loaded early in main.py before Typer processes args.
    # This function receives values from CLI args, envvars, or defaults (in that priority).
    config_file = config_module.find_config_file(config)
    if config_file:
        console.print(f"[dim]Using configuration from: {config_file}[/]")

    # Validate required cloud provider credentials
    provider_str = provider.value if provider else os.environ.get("NASIKO_PROVIDER")
    if provider_str:
        missing_creds = config_module.validate_required_credentials(provider_str)
        if missing_creds:
            console.print(f"[yellow]⚠️  Missing credentials for {provider_str}: {', '.join(missing_creds)}[/]")
            console.print(f"[yellow]   Set these in your .env file or export them as environment variables.[/]")

    # Ensure kubectl is available (will auto-download if not found)
    ensure_kubectl()

    # --- Validation ---
    # Either kubeconfig OR provider must be specified
    if kubeconfig is None and provider is None:
        console.print("[red]❌ You must specify either --kubeconfig (for existing cluster) or --provider (to create new cluster).[/]")
        console.print("\n[yellow]Examples:[/]")
        console.print("  # Use existing cluster:")
        console.print("  [cyan]--kubeconfig ./my-cluster-kubeconfig.yaml --registry-type cloud[/]")
        console.print("\n  # Create new cluster:")
        console.print("  [cyan]--provider digitalocean --registry-type cloud[/]")
        raise typer.Exit(1)

    # Track whether we're provisioning a new cluster or using existing
    provision_new_cluster = kubeconfig is None

    if kubeconfig and provider:
        console.print("[yellow]⚠️  Both --kubeconfig and --provider specified. Using existing cluster (--kubeconfig).[/]")
        # Keep provider value for registry configuration, but don't provision new cluster

    if registry_type == RegistryType.harbor:
        if not registry_pass:
            console.print("[red]❌ --registry-pass is required for Harbor setup.[/]")
            raise typer.Exit(1)
        # Domain and email are only required for external access with TLS
        if domain and not email:
            console.print("[red]❌ --email is required when --domain is specified for Harbor TLS certificates.[/]")
            raise typer.Exit(1)

    # --- STEP 1: Setup Kubernetes Connection ---
    if not provision_new_cluster:
        # Use existing cluster
        console.rule("[bold magenta]STEP 1: Connecting to Existing Cluster[/]")
        kubeconfig_path = Path(kubeconfig).resolve()

        if not kubeconfig_path.exists():
            console.print(f"[red]❌ Kubeconfig file not found: {kubeconfig_path}[/]")
            raise typer.Exit(1)

        # Set KUBECONFIG environment variable for all subsequent commands
        os.environ["KUBECONFIG"] = str(kubeconfig_path)

        # Fix kubeconfig permissions (should be 600 for security)
        import stat
        current_perms = os.stat(kubeconfig_path).st_mode
        if current_perms & (stat.S_IRWXG | stat.S_IRWXO):  # If group or other has any permissions
            try:
                os.chmod(kubeconfig_path, 0o600)  # Set to rw-------
                console.print(f"[dim]  Fixed kubeconfig permissions to 600 (owner read/write only)[/]")
            except Exception as e:
                console.print(f"[yellow]⚠️  Could not fix kubeconfig permissions: {e}[/]")

        console.print(f"[green]✅ Using kubeconfig: {kubeconfig_path}[/]")

        # Verify connection
        try:
            from kubernetes import config, client
            config.load_kube_config(config_file=str(kubeconfig_path))
            v1 = client.CoreV1Api()
            namespaces = v1.list_namespace()
            console.print(f"[green]✅ Connected to cluster ({len(namespaces.items)} namespaces found)[/]")
        except Exception as e:
            console.print(f"[red]❌ Failed to connect to cluster: {e}[/]")
            raise typer.Exit(1)

        # Clean up existing resources if requested
        if clean_existing:
            console.rule("[bold magenta]STEP 1.5: Cleaning Up Existing Resources[/]")
            console.print("[yellow]Removing existing Nasiko resources from cluster...[/]")

            # Call cleanup function with auto-approve
            from .utils import ensure_helm
            ensure_helm()  # Ensure helm is available for cleanup

            # Run cleanup logic inline (same as cleanup command)
            # Step 1: Delete Helm Releases
            console.print("[cyan]Removing Helm releases...[/]")
            helm_releases = [
                ("nasiko-auth", "nasiko"),
                ("kong", "nasiko"),
                ("mongodb", "nasiko"),
                ("redis", "nasiko")
            ]

            for release_name, namespace in helm_releases:
                try:
                    result = subprocess.run(
                        ["helm", "uninstall", release_name, "-n", namespace],
                        capture_output=True,
                        text=True,
                        timeout=30
                    )
                    if result.returncode == 0:
                        console.print(f"[green]  ✅ Removed: {release_name}[/]")
                except Exception:
                    pass  # Ignore errors during cleanup

            # Step 2: Delete Namespaces
            console.print("[cyan]Deleting namespaces...[/]")
            namespaces_to_delete = ["nasiko", "nasiko-agents", "buildkit","harbor","cert-manager","ingress-nginx"]

            for ns in namespaces_to_delete:
                try:
                    v1.delete_namespace(name=ns)
                    console.print(f"[green]  ✅ Deleted: {ns}[/]")
                except client.exceptions.ApiException as e:
                    if e.status == 404:
                        console.print(f"[dim]  ℹ️  {ns} not found[/]")
                except Exception:
                    pass  # Ignore errors

            # Wait for namespaces to be fully deleted
            console.print("[yellow]⏳ Waiting for namespaces to fully terminate...[/]")
            max_wait = 300  # Maximum 5 minutes
            wait_interval = 5  # Check every 5 seconds
            elapsed = 0

            while elapsed < max_wait:
                try:
                    remaining_namespaces = v1.list_namespace()
                    nasiko_namespaces = [ns.metadata.name for ns in remaining_namespaces.items
                                       if ns.metadata.name in namespaces_to_delete]

                    if not nasiko_namespaces:
                        console.print(f"[green]✅ All namespaces deleted ({elapsed}s)[/]")
                        break
                    else:
                        console.print(f"[dim]  Still terminating: {', '.join(nasiko_namespaces)} ({elapsed}s)[/]")
                        time.sleep(wait_interval)
                        elapsed += wait_interval
                except Exception:
                    break

            if elapsed >= max_wait:
                console.print("[yellow]⚠️  Timeout waiting for namespace deletion, proceeding anyway...[/]")

            # Extra buffer for cluster to stabilize
            console.print("[yellow]⏳ Waiting for cluster to stabilize (5s)...[/]")
            time.sleep(5)
            console.print("[green]✅ Cleanup complete[/]")

        # Determine region from provider if needed (for cloud registry)
        if not region and registry_type == RegistryType.cloud:
            if provider:
                # Set default region based on provider
                region = "nyc1" if provider == k8s_setup.Provider.digitalocean else "us-east-1"
                console.print(f"[yellow]⚠️  No region specified, using default for {provider.value}: {region}[/]")
            else:
                # Shouldn't reach here since we now require provider for cloud registry
                console.print("[red]❌ Region required when using cloud registry[/]")
                raise typer.Exit(1)

        buildkit_role_arn = None  # No IAM role for existing clusters

    else:
        # Provision new cluster
        console.rule("[bold magenta]STEP 1: Provisioning Kubernetes Cluster[/]")

        # Set default regions if not provided
        if not region:
            region = "nyc1" if provider == k8s_setup.Provider.digitalocean else "us-east-1"

        # Extract bundled terraform modules to ~/.nasiko/terraform/ (if not already present)
        try:
            setup_terraform_modules(source=terraform_dir, force=False)
        except FileNotFoundError as e:
            console.print(f"[red]❌ {e}[/]")
            raise typer.Exit(1)

        k8s_setup.create(
            provider=provider,
            cluster_name=cluster_name,
            region=region,
            auto_approve=True,
            terraform_dir=terraform_dir,
            state_dir=state_dir,
        )

        # Wait for the kubeconfig and nodes to stabilize
        console.print("[yellow]⏳ Waiting 15s for cluster nodes to stabilize...[/]")
        time.sleep(15)

        buildkit_role_arn = None
        if provider == k8s_setup.Provider.aws:
            # Get the working directory for terraform outputs
            from .terraform_state import get_cluster_state_info
            state_info = get_cluster_state_info(provider.value, cluster_name, state_dir)
            work_dir = state_info["work_dir"]

            # The key "buildkit_role_arn" must match your Terraform output name
            buildkit_role_arn = k8s_setup.get_tf_output(work_dir, "buildkit_role_arn")
            if buildkit_role_arn:
                console.print(f"[cyan]ℹ️  Found BuildKit IAM Role: {buildkit_role_arn}[/]")

    # --- STEP 2: Registry Setup ---
    console.rule(f"[bold magenta]STEP 2: Setting up {registry_type.value.upper()} Registry[/]")
    
    active_registry_url = ""
    active_username = ""
    active_password = ""

    if registry_type == RegistryType.harbor:
        # Deploy Harbor via Helm (domain/email optional for local setups)
        harbor_setup.deploy(
            domain=domain, 
            email=email, 
            password=registry_pass,
            username=registry_user
        )
        # Use internal cluster DNS for buildkit (more reliable than external domain)
        active_registry_url = "harbor-registry.harbor.svc.cluster.local:5000"
        active_username = registry_user
        active_password = registry_pass

        # Local Development Setup (when domain is not specified)
        if domain is None:
            console.print("[cyan]🏠 Detected local development setup - configuring Docker Desktop...[/]")

            # Configure Docker Desktop for Harbor registry access
            _configure_docker_desktop_for_harbor(registry_pass)

            # Setup Kubernetes registry credentials
            _setup_harbor_credentials(registry_user, registry_pass)

    elif registry_type == RegistryType.cloud:
        # Configure Cloud Registry (ECR/DOCR)
        if provider and provider == k8s_setup.Provider.aws and buildkit_role_arn:
            # For AWS with IAM roles, we only need the registry URL, not credentials
            # Get Account ID and construct ECR URL without fetching temporary credentials
            import json
            identity_json = subprocess.run(
                ["aws", "sts", "get-caller-identity", "--output", "json"],
                capture_output=True, text=True, check=True
            ).stdout.strip()
            account_id = json.loads(identity_json)["Account"]
            active_registry_url = f"{account_id}.dkr.ecr.{region}.amazonaws.com"
            active_username = None  # Not needed with IAM roles
            active_password = None  # Not needed with IAM roles

            # Ensure ECR repo exists
            subprocess.run(
                ["aws", "ecr", "create-repository", "--repository-name", cloud_reg_name, "--region", region],
                capture_output=True  # Ignore error if it already exists
            )
            console.print(f"[green]✅ Using AWS ECR with IAM role authentication: {active_registry_url}[/]")

            # Deploy ECR Refresher for namespaces that don't use IAM roles
            container_registry_setup.deploy_ecr_refresher(
                region=region,
                account_id=account_id,
                namespaces=["nasiko"]  # Only nasiko namespace needs the refresher, buildkit uses IAM
            )
        else:
            # For Harbor, DO, or AWS without IAM roles, get credentials normally
            # When using existing cluster, we need the provider to configure the registry
            if provider is None:
                console.print("[red]❌ --provider is required when using --registry-type cloud with existing cluster[/]")
                console.print("[yellow]   Specify --provider aws or --provider digitalocean[/]")
                raise typer.Exit(1)

            active_registry_url, active_username, active_password = container_registry_setup.deploy(
                provider=provider.value,
                region=region,
                name=cloud_reg_name
            )

            # Deploy ECR Refresher if using AWS (for namespaces needing token-based auth)
            if provider and provider == k8s_setup.Provider.aws:
                account_id = active_registry_url.split('.')[0]
                container_registry_setup.deploy_ecr_refresher(
                    region=region,
                    account_id=account_id,
                    namespaces=["nasiko", "buildkit"]
                )

    # --- STEP 3: Deploy Buildkit ---
    console.rule("[bold magenta]STEP 3: Deploying Buildkit Service[/]")
    # Buildkit needs credentials to push agent images to our registry
    buildkit_setup.deploy(
        registry=active_registry_url, 
        username=active_username, 
        password=active_password,
        iam_role_arn=buildkit_role_arn
    )

    # --- STEP 4: Deploy Nasiko Core Apps ---
    console.rule("[bold magenta]STEP 4: Deploying Nasiko Core Applications[/]")
    app_setup.deploy(
        registry_url=active_registry_url,
        registry_user=active_username,
        registry_pass=active_password,
        openai_key=openai_key,
        public_user=public_registry_user,
        superuser_username=superuser_username,
        superuser_email=superuser_email,
        provider=provider.value if provider else None,
        region=region
    )

    # --- STEP 5: Wait for Super User Creation ---
    console.rule("[bold magenta]STEP 5: Waiting for Super User Initialization[/]")

    try:
        from kubernetes import client as k8s_client
        import base64

        console.print("[yellow]⏳ Waiting for super user initialization job to complete...[/]")
        batch_v1 = k8s_client.BatchV1Api()
        core_v1 = k8s_client.CoreV1Api()

        # Wait for job to complete (max 5 minutes)
        max_wait = 60  # 5 minutes
        for i in range(max_wait):
            try:
                job = batch_v1.read_namespaced_job_status(name="superuser-init", namespace="nasiko")
                if job.status.succeeded and job.status.succeeded > 0:
                    console.print("[green]✅ Super user initialized successfully![/]")
                    break
                elif job.status.failed and job.status.failed > 0:
                    console.print("[yellow]⚠️  Super user job failed, checking if user already exists...[/]")
                    break
            except Exception:
                pass  # Job not found yet

            if i % 6 == 0:  # Print every 30 seconds
                console.print(f"[dim]  Still waiting... ({i*5}s elapsed)[/]")
            time.sleep(5)

        # Retrieve credentials from secret
        time.sleep(5)  # Small buffer for secret creation
        try:
            secret = core_v1.read_namespaced_secret(name="superuser-credentials", namespace="nasiko")

            # Decode credentials
            username = base64.b64decode(secret.data.get('username', '')).decode('utf-8')
            email = base64.b64decode(secret.data.get('email', '')).decode('utf-8')
            user_id = base64.b64decode(secret.data.get('user_id', '')).decode('utf-8')
            access_key = base64.b64decode(secret.data.get('access_key', '')).decode('utf-8')
            access_secret = base64.b64decode(secret.data.get('access_secret', '')).decode('utf-8')

            # Display credentials prominently
            console.print(f"\n[bold green]{'='*60}[/]")
            console.print(f"[bold green]  SUPER USER CREDENTIALS (SAVE THESE!)[/]")
            console.print(f"[bold green]{'='*60}[/]")
            console.print(f"  [bold]Username:[/]      [cyan]{username}[/]")
            console.print(f"  [bold]Email:[/]         [cyan]{email}[/]")
            console.print(f"  [bold]User ID:[/]       [dim]{user_id}[/]")
            console.print(f"  [bold]Access Key:[/]    [yellow]{access_key}[/]")
            console.print(f"  [bold]Access Secret:[/] [yellow]{access_secret}[/]")
            console.print(f"[bold green]{'='*60}[/]")
            console.print(f"\n[bold red]⚠️  Store the Access Secret securely - it won't be shown again![/]\n")

            # Save to local file as well
            from pathlib import Path
            import json
            creds_file = config_module.get_cluster_credentials_file(
                cluster_name=cluster_name,
                provider=provider.value if provider else None
            )
            with open(creds_file, 'w') as f:
                json.dump({
                    "username": username,
                    "email": email,
                    "user_id": user_id,
                    "access_key": access_key,
                    "access_secret": access_secret,
                    "created_at": time.strftime("%Y-%m-%d %H:%M:%S")
                }, f, indent=2)
            creds_file.chmod(0o600)
            console.print(f"[dim]📝 Credentials saved to: {creds_file}[/]\n")

        except Exception as e:
            console.print(f"[yellow]⚠️  Could not retrieve credentials from secret: {e}[/]")
            console.print(f"[yellow]   You can retrieve them later using:[/]")
            console.print(f"[cyan]   kubectl get secret superuser-credentials -n nasiko -o json | jq -r '.data | map_values(@base64d)'[/]\n")

    except Exception as e:
        console.print(f"[yellow]⚠️  Error during super user initialization: {e}[/]")
        console.print(f"[yellow]   Super user may not have been created. Check job logs:[/]")
        console.print(f"[cyan]   kubectl logs -n nasiko job/superuser-init[/]\n")


    # --- STEP 5.5: Deploy Kubernetes Dashboard ---
    console.rule("[bold magenta]STEP 5.5: Deploying Kubernetes Dashboard[/]")
    try:
        from pathlib import Path
        base_path = Path(__file__).parent.parent / "k8s"
        dashboard_yaml = base_path / "kube-dashboard.yaml"
        admin_yaml = base_path / "dashboard-admin.yaml"

        print(dashboard_yaml)
        print(admin_yaml)

        if dashboard_yaml.exists() and admin_yaml.exists():
            console.print("[cyan]Applying Kubernetes Dashboard manifests...[/]")
            subprocess.run(["kubectl", "apply", "-f", str(dashboard_yaml)], check=True, capture_output=True)
            console.print("[green]✅ Dashboard deployed[/]")
            
            console.print("[cyan]Creating Dashboard Admin User...[/]")
            subprocess.run(["kubectl", "apply", "-f", str(admin_yaml)], check=True, capture_output=True)
            console.print("[green]✅ Admin user created[/]")
        else:
            console.print(f"[yellow]⚠️  Dashboard manifests not found at {base_path}[/]")
    except Exception as e:
        console.print(f"[yellow]⚠️  Failed to deploy dashboard: {e}[/]")    

    # --- STEP 6: Fetch Access Points ---
    console.rule("[bold magenta]Finalizing Setup[/]")

    # Fetch IPs for the critical services
    # Note: Service names must match what is in app_setup.py and the helm chart
    agent_gateway_ip = get_service_external_ip("nasiko", "kong-gateway")


    #  Deploy Kubernetes Dashboard #

    # --- FINAL SUMMARY ---
    if registry_type == RegistryType.harbor and domain is None:
        # Local Development Summary
        console.print(f"""
    [bold green]🚀 NASIKO LOCAL DEVELOPMENT SETUP COMPLETE![/]
    
    [bold]Harbor Registry (Local):[/bold]
    • Web UI: http://localhost:30002
    • Registry: localhost:30500  
    • Username: {active_username}
    • Password: {active_password}
    
    [bold]Local Development Features:[/bold]
    ✅ Docker Desktop configured for Harbor access
    ✅ Harbor deployed with NodePort (30500, 30002)
    ✅ Kubernetes registry credentials configured
    ✅ Agent deployments will work seamlessly
    
    [bold]To Access Services:[/bold]
    Run the following command to enable port forwarding:
    [cyan]kubectl port-forward -n nasiko svc/kong-gateway 8000:80[/]
    
    [bold]Then access:[/bold]
    1. [cyan]Nasiko Web:[/] http://localhost:8000/app/
    2. [cyan]Nasiko API:[/] http://localhost:8000/api/v1
    3. [cyan]Nasiko Auth:[/] http://localhost:8000/auth
    4. [cyan]Nasiko Router:[/] http://localhost:8000/router
    
    [bold]Next Steps:[/bold]
    • Run port forwarding command above
    • Upload agents via the web UI: [cyan]http://localhost:8000/app/[/]
    • Use API: [cyan]http://localhost:8000/api/v1[/]
    • Check running agents: [cyan]kubectl get pods -n nasiko-agents[/]
    
    [bold]Kubernetes Dashboard:[/bold]
    • Access Dashboard: [cyan]kubectl port-forward -n kubernetes-dashboard svc/kubernetes-dashboard 8443:443[/]
    • URL: [cyan]https://localhost:8443[/]
    • Get Token: [cyan]kubectl -n kubernetes-dashboard create token admin-user[/]
    """)
    else:
        # Production/Cloud Summary
        console.print(f"""
    [bold green]🚀 NASIKO CLUSTER SETUP COMPLETE![/]
    
    [bold]Registry Configuration:[/bold]
    • Type: {registry_type.value}
    • URL:  {active_registry_url}
    
    [bold]Access Points:[/bold]
    1. [cyan]Nasiko Web:[/] http://{agent_gateway_ip}/app/ 
       
    2. [cyan]Nasiko Registry Api:[/] http://{agent_gateway_ip}/api/v1

    3. [cyan]Nasiko Auth Api:[/] http://{agent_gateway_ip}/auth

    4. [cyan]Nasiko Intent Based Router:[/] http://{agent_gateway_ip}/router

    [bold]Next Steps:[/bold]
    • If using Harbor, ensure DNS for [bold]{domain}[/] points to the Harbor Ingress IP.
    """)
if __name__ == "__main__":
    app()
