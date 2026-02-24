import sys
import json
import os
import textwrap
from typing import Optional
from .utils import ensure_aws_cli, ensure_doctl, ensure_kubectl
import typer
import subprocess
from rich.console import Console

app = typer.Typer(help="Setup Cloud Native Container Registry (ECR/DOCR)")
console = Console()

def _sanitize_do_token(token: Optional[str]) -> Optional[str]:
    t = (token or "").strip()
    if not t:
        return None
    # Be tolerant of dotenv/shell quoting mistakes.
    if len(t) >= 2 and t[0] == t[-1] and t[0] in ("'", '"'):
        t = t[1:-1].strip()
    return t or None


def _is_do_auth_error(text: str) -> bool:
    t = (text or "").lower()
    # doctl sometimes prints JSON errors to stdout, sometimes plain text to stderr.
    return (" 401 " in f" {t} " or "unauthorized" in t) and (
        "unable to authenticate you" in t
        or "invalid token" in t
        or "authentication" in t
        or "unauthorized" in t
    )


def _die_do_auth_hint():
    console.print("[red]❌ DigitalOcean authentication failed (doctl returned 401).[/]")
    console.print(
        "[yellow]   Check that DIGITALOCEAN_ACCESS_TOKEN is a valid DigitalOcean API token "
        "(not expired/revoked), or authenticate doctl via `doctl auth init`, then re-run.[/]"
    )
    console.print("[dim]   Tip: try `doctl account get` with the same token to validate it.[/]")
    sys.exit(1)

def _doctl_cmd(*args: str, token: Optional[str] = None) -> list[str]:
    """
    Build a doctl command.

    If token is provided, we pass it explicitly to avoid dependence on doctl contexts.
    If token is None/empty, we let doctl use its current auth context.
    """
    token = _sanitize_do_token(token)
    if not token:
        return ["doctl", *args]
    return ["doctl", "--access-token", token, *args]


def _get_digitalocean_token():
    """Resolve DO token from supported environment variable names."""
    return _sanitize_do_token(
        os.environ.get("DIGITALOCEAN_ACCESS_TOKEN")
        or os.environ.get("DO_TOKEN")
        or os.environ.get("TF_VAR_do_token")
    )


def _normalize_digitalocean_token_env():
    """
    Keep all supported token variable names in sync for Terraform/doctl compatibility.

    - doctl expects DIGITALOCEAN_ACCESS_TOKEN
    - Terraform module expects TF_VAR_do_token
    - Existing setup scripts historically use DO_TOKEN
    """
    token = _get_digitalocean_token()
    if not token:
        return None

    # Force consistency for subprocesses (doctl/terraform) within this run.
    os.environ["DIGITALOCEAN_ACCESS_TOKEN"] = token
    os.environ["DO_TOKEN"] = token
    os.environ["TF_VAR_do_token"] = token
    return token


def run_cmd(cmd_list, desc):
    """Runs a shell command and returns stdout."""
    try:
        result = subprocess.run(
            cmd_list, 
            check=True, 
            capture_output=True, 
            text=True
        )
        return result.stdout.strip()
    except subprocess.CalledProcessError as e:
        console.print(f"[red]❌ Error during: {desc}[/]")
        console.print(f"[dim]{e.stderr}[/]")
        sys.exit(1)
    except FileNotFoundError:
        console.print(f"[red]❌ Command not found: {cmd_list[0]}[/]")
        console.print(f"Please ensure {cmd_list[0]} is installed and in your PATH.")
        sys.exit(1)

def setup_aws_ecr(region: str, repo_name: str):
    """
    Sets up AWS ECR.
    1. Gets Account ID.
    2. Ensures Repository exists.
    3. Fetches temporary login password.
    """
    console.print(f"[cyan]Configuring AWS ECR for region: {region}...[/]")
    
    # 1. Get Account ID
    identity_json = run_cmd(
        ["aws", "sts", "get-caller-identity", "--output", "json"], 
        "AWS Identity Check"
    )
    account_id = json.loads(identity_json)["Account"]
    registry_url = f"{account_id}.dkr.ecr.{region}.amazonaws.com"
    
    # 2. Ensure Repo Exists
    # We check if it exists, if not create it.
    try:
        run_cmd(
            ["aws", "ecr", "describe-repositories", "--repository-names", repo_name, "--region", region],
            "Check ECR Repo"
        )
        console.print(f"[green]✅ Repository '{repo_name}' already exists.[/]")
    except SystemExit:
        # If describe fails (script exits), we actually want to catch that differently 
        # but run_cmd exits on error. Let's try creating directly.
        # Note: Ideally we change run_cmd to not exit, but for brevity:
        pass

    # Attempt create (idempotent if we ignore error, but AWS CLI errors if exists)
    # A safer quick check:
    console.print(f"[dim]Ensuring repository {repo_name} exists...[/]")
    subprocess.run(
        ["aws", "ecr", "create-repository", "--repository-name", repo_name, "--region", region],
        capture_output=True # Ignore output/error if it already exists
    )

    # 3. Get Password
    # Note: This token expires in 12 hours. In production, use an ECR Helper or Operator.
    password = run_cmd(
        ["aws", "ecr", "get-login-password", "--region", region],
        "Get ECR Login Password"
    )

    console.print("[yellow]⚠️  Note: AWS ECR tokens expire in 12 hours.[/]")
    
    # Return format for Nasiko
    return registry_url, "AWS", password


def deploy_ecr_refresher(region: str, account_id: str, namespaces: list = None):
    if namespaces is None:
        namespaces = ["nasiko", "buildkit"]
    """
    Deploys a CronJob that refreshes the AWS ECR token every 6 hours.
    It updates the 'regcred' secret in the specified namespaces.
    """
    # Ensure kubectl is available
    ensure_kubectl()

    console.print(f"[bold magenta]🔄 Deploying ECR Token Refresher for namespaces: {namespaces}[/]")

    # We use the official AWS CLI image and install kubectl on the fly to keep it lightweight & secure.
    # Note: Using 'bitnami/kubectl' and installing aws-cli is also an option, but this way is cleaner for AWS auth.
    image = "amazon/aws-cli:latest"
    
    # Logic:
    # 1. Get Token
    # 2. Delete old secret (if exists)
    # 3. Create new secret
    # We loop through all target namespaces.
    
    script = textwrap.dedent(f"""
        curl -LO "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl"
        chmod +x kubectl
        mv kubectl /usr/local/bin/

        TOKEN=$(aws ecr get-login-password --region {region})

        for ns in {' '.join(namespaces)}; do
            echo "Refreshing secret in namespace: $ns"
            kubectl delete secret regcred -n $ns --ignore-not-found
            kubectl create secret docker-registry regcred \\
                --docker-server={account_id}.dkr.ecr.{region}.amazonaws.com \\
                --docker-username=AWS \\
                --docker-password=$TOKEN \\
                -n $ns
        done
    """).strip()

    # 1. RBAC Manifests (Permissions to manage secrets)
    # We need a ClusterRole because we are managing secrets in multiple namespaces.
    rbac_manifest = """
apiVersion: v1
kind: ServiceAccount
metadata:
  name: ecr-refresher-sa
  namespace: default
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: ecr-token-manager-role
rules:
- apiGroups: [""]
  resources: ["secrets"]
  verbs: ["get", "delete", "create"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: ecr-token-manager-binding
subjects:
- kind: ServiceAccount
  name: ecr-refresher-sa
  namespace: default
roleRef:
  kind: ClusterRole
  name: ecr-token-manager-role
  apiGroup: rbac.authorization.k8s.io
"""

    # 2. CronJob Manifest
    # Indent the script properly for YAML (14 spaces for content after '|')
    indented_script = textwrap.indent(script, '              ')

    cron_manifest = f"""
apiVersion: batch/v1
kind: CronJob
metadata:
  name: ecr-credential-refresher
  namespace: default
spec:
  schedule: "0 */6 * * *" # Run every 6 hours
  successfulJobsHistoryLimit: 1
  failedJobsHistoryLimit: 2
  jobTemplate:
    spec:
      template:
        spec:
          serviceAccountName: ecr-refresher-sa
          containers:
          - name: refresher
            image: {image}
            command: ["/bin/bash", "-c"]
            args:
            - |
{indented_script}
          restartPolicy: OnFailure
"""

    # Apply Manifests
    try:
        # Apply RBAC
        subprocess.run(["kubectl", "apply", "-f", "-"], input=rbac_manifest, text=True, check=True, stdout=subprocess.DEVNULL)
        # Apply CronJob
        subprocess.run(["kubectl", "apply", "-f", "-"], input=cron_manifest, text=True, check=True, stdout=subprocess.DEVNULL)
        
        # Trigger the first job manually immediately so we don't have to wait 6 hours
        subprocess.run(
            ["kubectl", "create", "job", "--from=cronjob/ecr-credential-refresher", "ecr-refresher-init", "-n", "default"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        console.print("[green]✅ ECR Refresher CronJob deployed & triggered![/]")
        
    except subprocess.CalledProcessError as e:
        console.print(f"[red]❌ Failed to deploy ECR refresher: {e}[/]")

def setup_do_registry(registry_name: str):
    """
    Sets up DigitalOcean Container Registry (DOCR).
    Checks if a registry with the given name exists for the user. If not, creates it.
    """
    token = _normalize_digitalocean_token_env()
    if not token:
        console.print(
            "[red]❌ DigitalOcean token required. Set one of: "
            "DIGITALOCEAN_ACCESS_TOKEN, DO_TOKEN, or TF_VAR_do_token.[/]"
        )
        sys.exit(1)

    # Prefer explicit token to avoid doctl context issues.
    # If the provided token is stale, but doctl is authenticated via context, fall back to context.
    doctl_token: Optional[str] = token
    account_check = subprocess.run(
        _doctl_cmd("account", "get", "--output", "json", token=doctl_token),
        capture_output=True,
        text=True,
    )
    if account_check.returncode == 0 and account_check.stdout.strip():
        try:
            account_json = json.loads(account_check.stdout)
            if isinstance(account_json, list) and len(account_json) > 0:
                email = account_json[0].get("email")
            elif isinstance(account_json, dict):
                email = account_json.get("email")
            else:
                email = None

            if email:
                console.print(f"[dim]Authenticated as DO user: {email}[/]")
        except (json.JSONDecodeError, KeyError, IndexError, TypeError):
            console.print("[yellow]⚠️  Could not parse DO account details. Proceeding...[/]")
    else:
        err = (account_check.stderr or "").strip()
        out = (account_check.stdout or "").strip()
        if _is_do_auth_error(err) or _is_do_auth_error(out):
            # If doctl is already authenticated (context-based), allow that to work without requiring
            # the user to keep env tokens in sync.
            ctx_check = subprocess.run(
                _doctl_cmd("account", "get", "--output", "json", token=None),
                capture_output=True,
                text=True,
            )
            if ctx_check.returncode == 0 and (ctx_check.stdout or "").strip():
                console.print(
                    "[yellow]⚠️  DIGITALOCEAN_ACCESS_TOKEN appears invalid/stale, but doctl is authenticated via context.[/]"
                )
                console.print("[dim]   Continuing using the current doctl context token.[/]")
                doctl_token = None
                account_check = ctx_check
            else:
                _die_do_auth_hint()

        if account_check.returncode != 0:
            msg = err or (out.splitlines()[0] if out else "")
            if msg:
                console.print(f"[yellow]⚠️  Could not verify DO account details: {msg}[/]")
            else:
                console.print("[yellow]⚠️  Could not verify DO account details. Proceeding...[/]")

    console.print(f"[cyan]Configuring DigitalOcean Registry: {registry_name}...[/]")

    # First, check if the registry already exists by calling 'doctl registry get'
    # This returns the account's single registry (DO allows only one registry per account)
    registry_exists = False
    actual_name = None

    check_result = subprocess.run(
        _doctl_cmd("registry", "get", "--output", "json", token=doctl_token),
        _doctl_cmd("registry", "get", "--output", "json", token=doctl_token),
        capture_output=True,
        text=True
    )
    if check_result.returncode != 0:
        stderr = (check_result.stderr or "").strip()
        stdout = (check_result.stdout or "").strip()
        if _is_do_auth_error(stderr) or _is_do_auth_error(stdout):
            _die_do_auth_hint()
    if check_result.returncode != 0:
        stderr = (check_result.stderr or "").strip()
        stdout = (check_result.stdout or "").strip()
        if _is_do_auth_error(stderr) or _is_do_auth_error(stdout):
            _die_do_auth_hint()

    if check_result.returncode == 0 and check_result.stdout.strip():
        try:
            reg_info = json.loads(check_result.stdout)
            # doctl returns a list or a single object depending on version/output format
            if isinstance(reg_info, list) and len(reg_info) > 0:
                actual_name = reg_info[0].get("name")
            elif isinstance(reg_info, dict):
                actual_name = reg_info.get("name")

            if actual_name == registry_name:
                console.print(f"[green]✅ Using existing registry '{registry_name}'[/]")
                registry_exists = True
            else:
                console.print(f"[yellow]ℹ️  Found different registry '{actual_name}', but you requested '{registry_name}'[/]")
                console.print(f"[yellow]   DigitalOcean allows only ONE registry per account.[/]")
                console.print(f"[yellow]   To use your existing registry, specify: --cloud-reg-name {actual_name}[/]")
                console.print(f"[yellow]   Or delete the existing registry first: doctl registry delete --force[/]")
                sys.exit(1)
        except (json.JSONDecodeError, KeyError, IndexError) as e:
            console.print(f"[yellow]⚠️  Could not parse registry info: {e}[/]")
            console.print(f"[dim]   Will attempt to create registry '{registry_name}'...[/]")

    # If registry doesn't exist, try to create it
    if not registry_exists:
        console.print(f"[dim]Registry '{registry_name}' not found, attempting to create it...[/]")

        # Note: DigitalOcean registries are created globally but associated with a default region for data.
        # nyc3 is a safe default for DO registries.
        # We specify --subscription-tier professional to support multiple registries.
        create_cmd = _doctl_cmd(
            "registry",
            "create",
            registry_name,
            "--region",
            "nyc3",
            "--subscription-tier",
            "professional",
            token=doctl_token,
        )
        create_cmd = _doctl_cmd(
            "registry",
            "create",
            registry_name,
            "--region",
            "nyc3",
            "--subscription-tier",
            "professional",
            token=doctl_token,
        )

        create_result = subprocess.run(
            create_cmd,
            capture_output=True,
            text=True
        )

        if create_result.returncode == 0:
            console.print(f"[green]✅ Created new registry '{registry_name}'[/]")
        else:
            # Handle creation failures
            err_msg = create_result.stderr.strip()
            if _is_do_auth_error(err_msg) or _is_do_auth_error(create_result.stdout.strip()):
                _die_do_auth_hint()
            if _is_do_auth_error(err_msg) or _is_do_auth_error(create_result.stdout.strip()):
                _die_do_auth_hint()

            # Check if registry already exists (409 conflict)
            if "409" in err_msg or "already exists" in err_msg.lower() or "name already exists" in err_msg.lower():
                console.print(f"[green]✅ Registry '{registry_name}' already exists, using it[/]")
                # Continue execution - registry is available
            # Check for subscription plan error
            elif "invalid subscription plan" in err_msg.lower():
                console.print(f"[red]❌ Failed to create registry '{registry_name}' due to subscription plan limits.[/]")
                console.print(f"[yellow]   DigitalOcean error: {err_msg}[/]")
                if actual_name:
                    console.print(f"[cyan]   You have an existing registry: '{actual_name}'.[/]")
                    console.print(f"[cyan]   If you just upgraded your plan, it might take a few minutes to sync.[/]")
                sys.exit(1)
            else:
                # Real failure
                console.print(f"[red]❌ Failed to create registry '{registry_name}'.[/]")
                console.print(f"[dim]{err_msg}[/]")
                console.print(f"[yellow]   Hint: Registry names are global and must be unique across ALL DigitalOcean users.[/]")
                console.print(f"[yellow]   Try a more unique name if '{registry_name}' is taken by another user.[/]")
                sys.exit(1)
            
    registry_url = f"registry.digitalocean.com/{registry_name}"

    # 2. Get Credentials
    # Use 'doctl' to fetch a long-lived read-write docker credential.
    try:
        docker_config_str = run_cmd(
            [
                *_doctl_cmd("registry", "docker-config", token=doctl_token),
                *_doctl_cmd("registry", "docker-config", token=doctl_token),
                "--read-write",
                "--expiry-seconds", "31536000"  # 1 year
            ],
            "Get DO Docker Config (Read/Write)"
        )
        docker_config = json.loads(docker_config_str)
        
        auth_entry = docker_config.get("auths", {}).get("registry.digitalocean.com", {}).get("auth")
        
        if not auth_entry:
            raise ValueError("Could not find 'auth' token in doctl output.")

        import base64
        decoded = base64.b64decode(auth_entry).decode("utf-8")
        username, password = decoded.split(":", 1)

    except (SystemExit, ValueError, json.JSONDecodeError) as e:
        console.print(f"[yellow]⚠️  Could not parse credentials from 'doctl registry docker-config': {e}[/]")
        if doctl_token is None:
            console.print("[red]❌ Cannot fall back to DIGITALOCEAN_ACCESS_TOKEN because it appears invalid/stale.[/]")
            console.print("[dim]   Fix DIGITALOCEAN_ACCESS_TOKEN in your config, then re-run.[/]")
            sys.exit(1)
        console.print("[yellow]   Falling back to using your configured DigitalOcean token directly.[/]")
        if doctl_token is None:
            console.print("[red]❌ Cannot fall back to DIGITALOCEAN_ACCESS_TOKEN because it appears invalid/stale.[/]")
            console.print("[dim]   Fix DIGITALOCEAN_ACCESS_TOKEN in your config, then re-run.[/]")
            sys.exit(1)
        console.print("[yellow]   Falling back to using your configured DigitalOcean token directly.[/]")
        console.print("[dim]   Note: This token might have broad permissions.[/]")
        username = "access-token"
        password = token

    return registry_url, username, password

@app.command()
def deploy(
    provider: str = typer.Option(..., help="aws or digitalocean"),
    region: str = typer.Option(None, help="Region (Required for AWS)"),
    name: str = typer.Option(..., help="Registry Name (ECR Repo name or DO Registry Name)")
):
    """
    Configure Cloud Registry and return credentials (URL, User, Pass).

    Behavior:
    - DigitalOcean: Checks if you have access to the specified registry name.
                    If you have a different registry, will fail with clear instructions.
                    If no registry exists, creates new one.
    - AWS ECR:      Creates repository if it doesn't exist (multiple repos allowed).
    """
    if provider == "aws":
        ensure_aws_cli()  # <--- Ensure tool exists
        if not region:
            console.print("[red]❌ Region is required for AWS.[/]")
            sys.exit(1)
        url, user, pwd = setup_aws_ecr(region, name)
    
    elif provider == "digitalocean":
        ensure_doctl()    # <--- Ensure tool exists
        url, user, pwd = setup_do_registry(name)
    
    else:
        console.print(f"[red]❌ Unsupported provider: {provider}[/]")
        sys.exit(1)

    console.print(f"[green]✅ Cloud Registry Configured: {url}[/]")
    return url, user, pwd
# ...
if __name__ == "__main__":
    app()
