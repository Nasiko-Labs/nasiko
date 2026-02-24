import sys
import time
import subprocess
import typer
from kubernetes import client, config
from rich.console import Console
from rich.progress import Progress, SpinnerColumn, TextColumn

app = typer.Typer(help="Deploy Harbor stack using Helm & Python")
console = Console()

# --- Configuration: Helm Repos & Charts ---
CHARTS = {
    "ingress-nginx": {
        "repo_name": "ingress-nginx",
        "repo_url": "https://kubernetes.github.io/ingress-nginx",
        "chart": "ingress-nginx/ingress-nginx",
        "namespace": "ingress-nginx",
        "version": "4.10.0",
        "values": {
            "controller": {
                "service": {"type": "LoadBalancer"},
                "allowSnippetAnnotations": "true"
            }
        }
    },
    "cert-manager": {
        "repo_name": "jetstack",
        "repo_url": "https://charts.jetstack.io",
        "chart": "jetstack/cert-manager",
        "namespace": "cert-manager",
        "version": "v1.14.4",
        "values": {
            "installCRDs": "true"
        }
    },
    "harbor": {
        "repo_name": "harbor",
        "repo_url": "https://helm.goharbor.io",
        "chart": "harbor/harbor",
        "namespace": "harbor",
        "version": "1.14.0",
        "values": {} 
    }
}

def run_helm(args: list[str], description: str):
    from .utils import ensure_helm
    ensure_helm()
    """Wrapper to run helm commands with a spinner."""
    cmd = ["helm"] + args
    
    with Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        console=console
    ) as progress:
        task = progress.add_task(description, total=None)
        
        try:
            result = subprocess.run(
                cmd, 
                check=True, 
                capture_output=True, 
                text=True
            )
            progress.update(task, description=f"[green]✅ {description} - Done[/]")
            return result.stdout
        except subprocess.CalledProcessError as e:
            progress.update(task, description=f"[red]❌ {description} - Failed[/]")
            console.print(f"[bold red]Error Output:[/]\n{e.stderr}")
            sys.exit(1)

def add_repos():
    """Adds necessary Helm repositories."""
    console.rule("[bold cyan]1. Configuring Helm Repos[/]")
    for name, chart_config in CHARTS.items():
        run_helm(
            ["repo", "add", chart_config["repo_name"], chart_config["repo_url"]],
            f"Adding repo: {name}"
        )
    run_helm(["repo", "update"], "Updating Helm repositories")

def deploy_chart(name: str, release_name: str, values: dict):
    """Deploys or upgrades a Helm chart."""
    config = CHARTS[name]
    
    # Merge default values with dynamic values
    final_values = config["values"].copy()
    final_values.update(values)
    
    # Construct --set arguments flattened
    set_args = []
    for key, val in flatten_dict(final_values).items():
        set_args.extend(["--set", f"{key}={val}"])

    cmd = [
        "upgrade", "--install", release_name, config["chart"],
        "--namespace", config["namespace"],
        "--create-namespace",
        "--version", config["version"],
        "--wait"  # Wait for pods to be ready
    ] + set_args

    run_helm(cmd, f"Deploying {name} ({config['version']})")

def flatten_dict(d, parent_key='', sep='.'):
    """Flattens a nested dictionary for Helm --set notation."""
    items = []
    for k, v in d.items():
        safe_k = k.replace(".", "\\.")
        new_key = f"{parent_key}{sep}{safe_k}" if parent_key else safe_k
        if isinstance(v, dict):
            items.extend(flatten_dict(v, new_key, sep=sep).items())
        else:
            items.append((new_key, v))
    return dict(items)

def get_ingress_ip():
    """Polls the Ingress Controller Service for the External IP/Hostname."""
    try:
        config.load_kube_config()
        v1 = client.CoreV1Api()
    except Exception:
        return "[red]Unknown (Could not load kubeconfig)[/]"

    namespace = "ingress-nginx"
    service_name = "ingress-nginx-controller"

    with console.status("[bold yellow]⏳ Waiting for LoadBalancer IP assignment...[/]"):
        # Retry for up to 60 seconds
        for _ in range(30):
            try:
                svc = v1.read_namespaced_service(name=service_name, namespace=namespace)
                ingress = svc.status.load_balancer.ingress
                if ingress:
                    # Return IP (DigitalOcean/GKE) or Hostname (AWS)
                    return ingress[0].ip or ingress[0].hostname
            except Exception:
                pass
            time.sleep(2)
    
    return "[red]Pending (Run 'kubectl get svc -n ingress-nginx')[/]"

def create_cluster_issuer(email: str):
    """Applies the ClusterIssuer YAML using kubectl (via subprocess)."""
    manifest = f"""
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: letsencrypt-prod
spec:
  acme:
    server: https://acme-v02.api.letsencrypt.org/directory
    email: {email}
    privateKeySecretRef:
      name: letsencrypt-prod-key
    solvers:
    - http01:
        ingress:
          class: nginx
"""
    run_process = subprocess.Popen(
        ["kubectl", "apply", "-f", "-"], 
        stdin=subprocess.PIPE, 
        stdout=subprocess.PIPE, 
        stderr=subprocess.PIPE,
        text=True
    )
    stdout, stderr = run_process.communicate(input=manifest)
    
    if run_process.returncode != 0:
        console.print(f"[red]Failed to create ClusterIssuer:[/]\n{stderr}")
    else:
        console.print("[green]✅ ClusterIssuer created[/]")

@app.command()
def deploy(
    domain: str = typer.Option(None, help="Domain Name (e.g., reg.example.com) - optional for local setups"),
    email: str = typer.Option(None, help="Email for Let's Encrypt - required only when domain is provided"),
    password: str = typer.Option(..., help="Harbor Admin Password"),
    username: str = typer.Option("admin", help="Harbor Admin Username")
):
    """Deploys the full stack using Helm."""
    
    # 1. Add Repos
    add_repos()

    # 2. Deploy NGINX Ingress
    console.rule("[bold cyan]2. Deploying Infrastructure[/]")
    deploy_chart("ingress-nginx", "ingress-nginx", {})

    # 3. Deploy Cert Manager
    deploy_chart("cert-manager", "cert-manager", {})

    # 4. Deploy Harbor
    console.rule("[bold cyan]3. Deploying Harbor[/]")
    
    # Base Harbor values with configurable admin credentials
    base_harbor_values = {
        "harborAdminPassword": password,
        # Configure registry to use the same admin credentials
        "registry": {
            "credentials": {
                "username": username,
                "password": password
            }
        }
    }
    
    if domain:
        # External access with Ingress and TLS
        harbor_values = {
            **base_harbor_values,
            "expose": {
                "type": "ingress", 
                "tls": {
                    "enabled": "true",
                    "certSource": "secret",
                    "secret": {
                        "secretName": "harbor-tls"
                    }
                },
                "ingress": {
                    "hosts": {"core": domain},
                    "annotations": {
                        "cert-manager.io/cluster-issuer": "letsencrypt-prod",
                        "kubernetes.io/ingress.class": "nginx",
                        "nginx.ingress.kubernetes.io/ssl-redirect": '"true"'
                    }
                }
            },
            "externalURL": f"https://{domain}"
        }
    else:
        # Local cluster access - use NodePort for Docker Desktop compatibility
        harbor_values = {
            **base_harbor_values,
            "expose": {
                "type": "nodePort",
                "tls": {
                    "enabled": "false"
                },
                "nodePort": {
                    "ports": {
                        "http": {"port": 80, "nodePort": 30002},
                        "https": {"port": 443, "nodePort": 30003}  
                    }
                }
            },
            "registry": {
                **base_harbor_values.get("registry", {}),
                "nodePort": 30500
            },
            "externalURL": "http://localhost:30002"
        }
    
    deploy_chart("harbor", "harbor", harbor_values)

    # For local setups, create NodePort service for registry access
    if not domain:
        console.print("[yellow]⏳ Creating NodePort service for Harbor registry...[/]")
        try:
            # Create NodePort service for Harbor registry on port 30500
            subprocess.run([
                "kubectl", "expose", "service", "harbor-registry", 
                "--type=NodePort", "--name=harbor-registry-nodeport", 
                "--port=5000", "--target-port=5000", "-n", "harbor"
            ], capture_output=True, check=True)
            
            # Patch to set specific nodePort
            subprocess.run([
                "kubectl", "patch", "svc", "harbor-registry-nodeport", "-n", "harbor",
                "-p", '{"spec":{"ports":[{"port":5000,"nodePort":30500,"targetPort":5000}]}}'
            ], capture_output=True, check=True)
            
            console.print("[green]✅ Harbor registry NodePort service created on port 30500[/]")
        except subprocess.CalledProcessError as e:
            console.print(f"[yellow]⚠️  Could not create NodePort service: {e}[/]")
            console.print("[yellow]   You may need to create it manually for agent deployments[/]")

    if domain:
        # 5. Create ClusterIssuer (only for external access)
        console.rule("[bold cyan]4. Finalizing Configuration[/]")
        create_cluster_issuer(email)

        # 6. Get LoadBalancer IP
        lb_ip = get_ingress_ip()

        console.print(f"""
        [bold green]✅ Deployment Complete![/]
        
        1. DNS: Point [bold]{domain}[/] to: [bold cyan]{lb_ip}[/]
        2. Access: https://{domain}
        """)
    else:
        console.print(f"""
        [bold green]✅ Harbor Deployed for Local Access![/]
        
        • Harbor Core: http://localhost:30002
        • Harbor Registry: localhost:30500  
        • Username: {username}
        • Password: {password}
        • Access via NodePort from host machine
        """)

if __name__ == "__main__":
    app()
