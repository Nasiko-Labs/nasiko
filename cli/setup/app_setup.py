import sys
import os
import base64
import secrets
import urllib.parse
import secrets
import urllib.parse
import json
import typer
import yaml
import subprocess
from pathlib import Path
from kubernetes import client, config
from rich.console import Console
from .buildkit_setup import apply_manifest
from .config import (
    DEFAULT_PUBLIC_REGISTRY_USER, 
    save_cluster_info
)

app = typer.Typer(help="Deploy Nasiko Core Apps using YAML manifests + Python secrets")
console = Console()

# --- Configuration ---
NAMESPACE = "nasiko"
AGENTS_NAMESPACE = "nasiko-agents"

class ManifestLoader:
    """Loads and processes YAML manifests with template variable injection."""

    def __init__(self, charts_dir):
        self.charts_dir = Path(charts_dir)
        if not self.charts_dir.exists():
            console.print(f"[red]❌ Charts directory not found: {self.charts_dir}[/]")
            sys.exit(1)

    def load_yaml(self, relative_path, **template_vars):
        """Load YAML file and replace template variables."""
        file_path = self.charts_dir / relative_path

        if not file_path.exists():
            console.print(f"[red]❌ YAML file not found: {file_path}[/]")
            sys.exit(1)

        try:
            with open(file_path, 'r') as f:
                content = f.read()

            # Replace placeholders like {{ REGISTRY_URL }}
            for key, value in template_vars.items():
                placeholder = f"{{{{ {key} }}}}"
                content = content.replace(placeholder, str(value))

            return yaml.safe_load(content)

        except Exception as e:
            console.print(f"[red]❌ Error loading YAML {file_path}: {e}[/]")
            sys.exit(1)

    def inject_env_vars(self, deployment_yaml, env_vars):
        """Inject environment variables into deployment YAML."""
        try:
            containers = deployment_yaml["spec"]["template"]["spec"]["containers"]
            if containers:
                # Convert env_vars dict to Kubernetes env format
                env_list = [{"name": k, "value": str(v)} for k, v in env_vars.items()]

                # Merge with existing env vars if any
                existing_env = containers[0].get("env", [])

                # Create a dict to avoid duplicates (new values override existing)
                env_dict = {item["name"]: item["value"] for item in existing_env}
                env_dict.update({item["name"]: item["value"] for item in env_list})

                # Convert back to list format
                containers[0]["env"] = [{"name": k, "value": v} for k, v in env_dict.items()]

        except KeyError as e:
            console.print(f"[red]❌ Invalid deployment YAML structure: missing {e}[/]")
            sys.exit(1)

    def inject_image_override(self, deployment_yaml, image_url):
        """Override the container image in deployment YAML."""
        try:
            containers = deployment_yaml["spec"]["template"]["spec"]["containers"]
            if containers:
                containers[0]["image"] = image_url
        except KeyError as e:
            console.print(f"[red]❌ Invalid deployment YAML structure: missing {e}[/]")
            sys.exit(1)


class SecretManager:
    """Manages Kubernetes secrets generation using pure Python."""

    def __init__(self, k8s_client, namespace):
        self.k8s_client = k8s_client
        self.namespace = namespace

    def create_registry_secret(self, registry, username, password, secret_name="regcred"):
        """Generate Docker registry secret dynamically."""
        registry_domain = registry.split('/')[0]

        auth_str = f"{username}:{password}"
        auth_b64 = base64.b64encode(auth_str.encode("utf-8")).decode("utf-8")

        docker_config = {
            "auths": {
                registry_domain: {
                    "auth": auth_b64
                }
            }
        }

        manifest = {
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": {
                "name": secret_name,
                "namespace": self.namespace
            },
            "type": "kubernetes.io/dockerconfigjson",
            "stringData": {
                ".dockerconfigjson": json.dumps(docker_config)
            }
        }

        apply_manifest(self.k8s_client, manifest, f"Registry Secret ({secret_name}) in {self.namespace}")

    def create_app_secrets(self, **secret_vars):
        """Generate application secrets from provided variables."""
        if not secret_vars:
            console.print("[yellow]ℹ️  No app secrets to create[/]")
            return

        secrets_data = {k: str(v) for k, v in secret_vars.items() if v is not None}

        if secrets_data:
            manifest = {
                "apiVersion": "v1",
                "kind": "Secret",
                "metadata": {
                    "name": "app-secrets",
                    "namespace": self.namespace
                },
                "type": "Opaque",
                "stringData": secrets_data
            }

            apply_manifest(self.k8s_client, manifest, f"App Secrets in {self.namespace}")


class NasikoDeployer:
    """Main deployer class that orchestrates YAML-based deployment with Python secrets."""

    def __init__(self, k8s_client, registry_config, environment="default", provider=None, region=None):
        self.k8s_client = k8s_client
        self.environment = environment
        self.registry = registry_config
        self.provider = provider
        self.region = region

        # Initialize manifest loader
        charts_dir = Path(__file__).parent.parent / "k8s" / "charts" / "nasiko-platform" / "templates"
        self.loader = ManifestLoader(charts_dir)

        # Initialize secret managers
        self.secrets = SecretManager(k8s_client, NAMESPACE)
        self.agent_secrets = SecretManager(k8s_client, AGENTS_NAMESPACE)

        # Store gateway URL after deployment
        self.gateway_url = None
        self.global_template_vars = {}

    def get_or_create_static_ip(self):
        """Get LoadBalancer IP configuration for the cloud provider.

        Note: Both AWS and DigitalOcean use dynamic IPs for LoadBalancer services.
        - AWS: In-tree controller doesn't support EIP allocation; needs AWS LB Controller
        - DigitalOcean: Reserved IPs cannot be assigned to LoadBalancer services

        Returns provider info for setting appropriate LoadBalancer annotations.
        """
        if not self.provider or not self.region:
            console.print("[dim]No provider/region specified, using default LoadBalancer config[/]")
            return None

        if self.provider == "digitalocean":
            return self._handle_digitalocean_ip()
        elif self.provider == "aws":
            return self._handle_aws_ip()
        else:
            console.print(f"[yellow]Unknown provider '{self.provider}', using default LoadBalancer config[/]")
            return None

    def _handle_digitalocean_ip(self):
        """Configure DigitalOcean LoadBalancer.

        Note: DigitalOcean does not support assigning reserved IPs to LoadBalancer
        services. The LoadBalancer will get a dynamic public IP.
        """
        console.print("[cyan]DigitalOcean: Using dynamic LoadBalancer IP[/]")
        console.print("[dim]   Note: DO does not support reserved IPs for LoadBalancers[/]")
        return {"provider": "digitalocean", "ip": None, "dynamic": True}

    def _handle_aws_ip(self):
        """Configure AWS LoadBalancer (NLB).

        Note: AWS in-tree LoadBalancer controller doesn't support EIP allocation.
        Using AWS Load Balancer Controller addon would enable static IPs,
        but for simplicity we use dynamic NLB IP assignment.
        """
        console.print("[cyan]AWS: Using dynamic LoadBalancer IP (NLB)[/]")
        console.print("[dim]   Note: For static IP, install AWS Load Balancer Controller addon[/]")
        return {"provider": "aws", "ip": None, "dynamic": True}

    def deploy_namespaces(self):
        """Deploy namespaces first - prerequisite for all other resources."""
        console.rule("[bold magenta]0. Namespaces[/]")
        try:
            namespace_path = self.loader.charts_dir / "namespace.yaml"
            with open(namespace_path, 'r') as f:
                for doc in yaml.safe_load_all(f):
                    if doc:  # Skip empty documents
                        apply_manifest(self.k8s_client, doc, f"Namespace: {doc['metadata']['name']}")
        except Exception as e:
            console.print(f"[red]❌ Error creating namespaces: {e}[/]")
            raise

    def deploy_infrastructure(self, **template_vars):
        """Deploy infrastructure using existing YAML manifests."""
        console.rule("[bold magenta]1. Infrastructure (Redis, Mongo, PostgreSQL, Ollama)[/]")
        self._deploy_infra_components(**template_vars)

    def _deploy_infra_components(self, **template_vars):
        """Deploy infrastructure components from YAML files."""
        infra_components = [
            "infrastructure/redis.yaml",
            "infrastructure/mongodb.yaml",
            "infrastructure/postgresql.yaml",
            "infrastructure/ollama.yaml",
            "infrastructure/phoenix.yaml",
            "infrastructure/ollama.yaml",
            "infrastructure/phoenix.yaml",
        ]

        for component_path in infra_components:
            try:
                component_name = component_path.split('/')[-1].replace('.yaml', '')
                file_path = self.loader.charts_dir / component_path

                with open(file_path, 'r') as f:
                    content = f.read()

                # Replace placeholders
                for key, value in template_vars.items():
                    placeholder = f"{{{{ {key} }}}}"
                    content = content.replace(placeholder, str(value))


                manifests = list(yaml.safe_load_all(content))

                # Deploy PVCs first and wait for them to be bound
                pvcs_to_wait = []
                for doc in manifests:
                    if doc and doc.get('kind') == 'PersistentVolumeClaim':
                        resource_name = doc.get('metadata', {}).get('name', component_name)
                        apply_manifest(self.k8s_client, doc, f"Infrastructure PVC: {resource_name}")
                        pvcs_to_wait.append((doc.get('metadata', {}).get('name'), doc.get('metadata', {}).get('namespace', 'default')))

                # Wait for PVCs to be bound before proceeding
                if pvcs_to_wait:
                    self._wait_for_pvcs_bound(pvcs_to_wait, component_name)

                # Deploy other resources
                for doc in manifests:
                    if doc and doc.get('kind') != 'PersistentVolumeClaim':  # Skip PVCs as they're already deployed
                        resource_name = doc.get('metadata', {}).get('name', component_name)
                        apply_manifest(self.k8s_client, doc, f"Infrastructure: {resource_name}")

            except Exception as e:
                console.print(f"[yellow]⚠️  Warning: Could not deploy {component_path}: {e}[/]")

    def _wait_for_pvcs_bound(self, pvcs_to_wait, component_name, timeout=300):
        """Wait for PVCs to be bound before proceeding with deployment."""
        import time

        if not pvcs_to_wait:
            return

        console.print(f"[cyan]⏳ Waiting for {len(pvcs_to_wait)} PVC(s) in {component_name} to be bound...[/]")
        v1 = client.CoreV1Api()
        start_time = time.time()
        check_interval = 5

        while pvcs_to_wait and (time.time() - start_time) < timeout:
            remaining_pvcs = []

            for pvc_name, pvc_namespace in pvcs_to_wait:
                try:
                    pvc = v1.read_namespaced_persistent_volume_claim(name=pvc_name, namespace=pvc_namespace)
                    if pvc.status.phase == 'Bound':
                        console.print(f"[green]✅ PVC {pvc_name} is bound[/]")
                    else:
                        console.print(f"[dim]   PVC {pvc_name} status: {pvc.status.phase}[/]")
                        remaining_pvcs.append((pvc_name, pvc_namespace))
                except client.exceptions.ApiException as e:
                    if e.status == 404:
                        console.print(f"[yellow]⚠️  PVC {pvc_name} not found yet[/]")
                        remaining_pvcs.append((pvc_name, pvc_namespace))
                    else:
                        console.print(f"[yellow]⚠️  Error checking PVC {pvc_name}: {e}[/]")
                        remaining_pvcs.append((pvc_name, pvc_namespace))

            pvcs_to_wait = remaining_pvcs

            if pvcs_to_wait:
                time.sleep(check_interval)
            else:
                console.print(f"[green]✅ All PVCs for {component_name} are bound[/]")
                return

        if pvcs_to_wait:
            console.print(f"[yellow]⚠️  Timeout waiting for PVCs to bind: {[pvc[0] for pvc in pvcs_to_wait]}[/]")
            console.print(f"[yellow]   Proceeding anyway, but deployment may fail if storage isn't ready[/]")

    def deploy_rbac(self):
        """Deploy RBAC from YAML files."""
        console.rule("[bold magenta]2. RBAC Configuration[/]")

        rbac_files = [
            "rbac/serviceaccount.yaml",
            "rbac/clusterrole.yaml",
            "rbac/clusterrolebinding.yaml"
        ]

        for file_path in rbac_files:
            try:
                full_path = self.loader.charts_dir / file_path
                with open(full_path, 'r') as f:
                    for doc in yaml.safe_load_all(f):
                        if doc:  # Skip empty documents
                            apply_manifest(self.k8s_client, doc, f"RBAC: {doc['metadata']['name']}")
            except Exception as e:
                console.print(f"[yellow]⚠️  Warning: Could not load {file_path}: {e}[/]")

        # Apply agent-rbac for cross-namespace permissions
        self._apply_agent_rbac()

    def _apply_agent_rbac(self):
        """Apply agent RBAC for cross-namespace permissions."""
        agent_rbac_path = Path(__file__).parent.parent / "k8s" / "agent-rbac.yaml"

        if agent_rbac_path.exists():
            console.print("[cyan]Applying agent RBAC for nasiko-agents namespace...[/]")
            try:
                with open(agent_rbac_path) as f:
                    for doc in yaml.safe_load_all(f):
                        if doc:
                            apply_manifest(self.k8s_client, doc, f"Agent RBAC: {doc['metadata']['name']}")
            except Exception as e:
                console.print(f"[yellow]⚠️  Warning: Error applying agent RBAC: {e}[/]")
        else:
            console.print("[yellow]⚠️  Warning: agent-rbac.yaml not found, skipping cross-namespace permissions[/]")

    def deploy_secrets(self, registry_user=None, registry_pass=None, **secret_vars):
        """Deploy all secrets using Python generation."""
        console.rule("[bold magenta]3. Secrets & Registry Credentials[/]")

        # Registry secrets
        if registry_user and registry_pass:
            self.secrets.create_registry_secret(
                self.registry["url"], registry_user, registry_pass, "regcred"
            )
            self.agent_secrets.create_registry_secret(
                self.registry["url"], registry_user, registry_pass, "agent-registry-credentials"
            )
        else:
            console.print("[yellow]ℹ️  No registry credentials provided, skipping registry secrets[/]")

        # Application secrets
        if secret_vars:
            self.secrets.create_app_secrets(**secret_vars)

    def deploy_core_services(self):
        """Deploy core services from YAML with dynamic injection."""
        console.rule("[bold magenta]4. Core Services (Backend, Web)[/]")

        services = [
            {
                "yaml_path": "services/nasiko-backend/deployment.yaml",
                "name": "Backend",
                "template_vars": {
                    "PUBLIC_REGISTRY": self.registry["public"]
                },
                "env_vars": {
                    "MONGO_NASIKO_HOST": "mongodb",
                    "MONGO_NASIKO_PORT": "27017",
                    "MONGO_NASIKO_DATABASE": "nasiko",
                    "REDIS_HOST": "redis-master",
                    "BUILDKIT_HOST": "tcp://buildkitd.buildkit.svc.cluster.local:1234",
                    "REGISTRY_URL": self.registry["url"],
                    "IMAGE_PULL_SECRET": "regcred"
                },
                "service_port": 8000
            },
            {
                "yaml_path": "services/nasiko-web/deployment.yaml",
                "name": "Web",
                "template_vars": {
                    "PUBLIC_REGISTRY": self.registry["public"]
                },
                "env_vars": {
                    "NODE_ENV": "production",
                    "API_BASE_URL": "/api/v1",
                    "CHAT_SERVICE_URL": "/api/v1",
                    "ROUTER_URL": "/router",
                    "IS_DEVELOPMENT": "false"
                },
                "service_port": 4000
            }
        ]


        # Add DO token to backend if available
        do_token = (
            os.getenv("DIGITALOCEAN_ACCESS_TOKEN")
            or os.getenv("DO_TOKEN")
            or os.getenv("TF_VAR_do_token")
        )

        # Add DO token to backend if available
        do_token = (
            os.getenv("DIGITALOCEAN_ACCESS_TOKEN")
            or os.getenv("DO_TOKEN")
            or os.getenv("TF_VAR_do_token")
        )
        if do_token:
            services[0]["env_vars"]["DO_TOKEN"] = do_token

        for service in services:
            self._deploy_service(service)

    def deploy_k8s_build_worker(self):
        """Deploy K8s Build Worker for BuildKit-based agent builds."""
        console.rule("[bold magenta]4.5. K8s Build Worker[/]")

        worker_config = {
            "yaml_path": "services/nasiko-k8s-build-worker/deployment.yaml",
            "name": "K8s Build Worker",
            "template_vars": {
                "PUBLIC_REGISTRY": self.registry["public"]
            },
            "env_vars": {
                "MONGO_NASIKO_HOST": "mongodb",
                "MONGO_NASIKO_PORT": "27017",
                "MONGO_NASIKO_DATABASE": "nasiko",
                "REDIS_HOST": "redis-master",
                "REDIS_PORT": "6379",
                "REDIS_DB": "0",
                "BUILDKIT_ADDRESS": "tcp://buildkitd.buildkit.svc.cluster.local:1234",
                "REGISTRY_URL": self.registry["url"],
                "AUTH_SERVICE_URL": "http://nasiko-auth.nasiko.svc.cluster.local:8001",
                "ENV": "production"
            }
        }

        # Add GATEWAY_URL if available (set after gateway deployment)
        if self.gateway_url:
            worker_config["env_vars"]["GATEWAY_URL"] = self.gateway_url
            console.print(f"[cyan]   Using gateway URL: {self.gateway_url}[/]")
        else:
            console.print("[yellow]⚠️  Warning: Gateway URL not set, worker will need manual configuration[/]")

        # Add DO token for registry auth
        do_token = (
            os.getenv("DIGITALOCEAN_ACCESS_TOKEN")
            or os.getenv("DO_TOKEN")
            or os.getenv("TF_VAR_do_token")
        )
        # Add DO token for registry auth
        do_token = (
            os.getenv("DIGITALOCEAN_ACCESS_TOKEN")
            or os.getenv("DO_TOKEN")
            or os.getenv("TF_VAR_do_token")
        )
        if do_token:
            worker_config["env_vars"]["DO_TOKEN"] = do_token

        # Add OPENAI_API_KEY for agents that need LLM access
        openai_key = os.getenv("OPENAI_API_KEY")
        if openai_key:
            worker_config["env_vars"]["OPENAI_API_KEY"] = openai_key

        try:
            self._deploy_service(worker_config)
            console.print("[green]✅ K8s Build Worker deployed (handles agent builds via BuildKit)[/]")
        except Exception as e:
            console.print(f"[yellow]⚠️  Warning: Could not deploy K8s Build Worker: {e}[/]")
            console.print("[yellow]   Agent builds will not work without the worker[/]")

    def deploy_n8n(self):
        """Deploy N8N automation service with gateway URL configuration."""
        console.rule("[bold magenta]6.8. N8N Automation Service[/]")

        # Deploy N8N PVC first
        n8n_pvc_config = {
            "yaml_path": "services/n8n/pvc.yaml",
            "name": "N8N Storage"
        }
        self._deploy_service(n8n_pvc_config)

        # Deploy N8N with environment variables
        n8n_config = {
            "yaml_path": "services/n8n/deployment.yaml",
            "name": "N8N Deployment",
            "env_vars": {}
        }

        # Set gateway URLs if available
        if self.gateway_url:
            n8n_config["env_vars"]["N8N_EDITOR_BASE_URL"] = self.gateway_url
            n8n_config["env_vars"]["WEBHOOK_URL"] = f"{self.gateway_url}/n8n"
            console.print(f"[cyan]   Using N8N Editor URL: {self.gateway_url}[/]")
            console.print(f"[cyan]   Using Webhook URL: {self.gateway_url}/n8n[/]")
        else:
            console.print("[yellow]⚠️  Warning: Gateway URL not set, n8n will need manual configuration[/]")

        try:
            self._deploy_service(n8n_config)

            # Deploy N8N service
            n8n_service_config = {
                "yaml_path": "services/n8n/service.yaml",
                "name": "N8N Service"
            }
            self._deploy_service(n8n_service_config)

            console.print("[green]✅ N8N automation service deployed[/]")
            if self.gateway_url:
                console.print(f"[cyan]   N8N Editor accessible at: {self.gateway_url}/n8n[/]")
        except Exception as e:
            console.print(f"[yellow]⚠️  Warning: Could not deploy N8N: {e}[/]")
            console.print("[yellow]   N8N automation service is optional[/]")

    def _get_available_storage_class(self):
        """Detect available storage class based on cluster type."""
        try:
            # Get available storage classes
            v1_storage = client.StorageV1Api()
            storage_classes = v1_storage.list_storage_class()

            available_classes = [sc.metadata.name for sc in storage_classes.items]
            console.print(f"[dim]Available storage classes: {available_classes}[/]")

            # Priority order: cloud-specific first, then local fallbacks
            priority_order = [
                "do-block-storage",  # DigitalOcean
                "gp2", "gp3",        # AWS
                "premium-rwo",       # GCP
                "hostpath",          # Local/Docker Desktop
                "local-path"         # k3s/local
            ]

            for preferred in priority_order:
                if preferred in available_classes:
                    console.print(f"[green]Using storage class: {preferred}[/]")
                    return preferred

            # Use default if available
            for sc in storage_classes.items:
                if sc.metadata.annotations and sc.metadata.annotations.get('storageclass.kubernetes.io/is-default-class') == 'true':
                    console.print(f"[green]Using default storage class: {sc.metadata.name}[/]")
                    return sc.metadata.name

            console.print("[yellow]⚠️  No suitable storage class found, using first available[/]")
            return available_classes[0] if available_classes else None

        except Exception as e:
            console.print(f"[yellow]⚠️  Could not detect storage class: {e}[/]")
            return None

    def _deploy_service(self, service_config):
        """Deploy a single service with its deployment and service."""
        try:
            # Load YAML file with template variables
            template_vars = self.global_template_vars.copy()
            template_vars.update(service_config.get("template_vars", {}))
            file_path = self.loader.charts_dir / service_config["yaml_path"]

            with open(file_path, 'r') as f:
                content = f.read()

            # Replace template variables
            for key, value in template_vars.items():
                placeholder = f"{{{{ {key} }}}}"
                content = content.replace(placeholder, str(value))

            # Load all documents from the YAML file
            manifests = list(yaml.safe_load_all(content))

            deployment_yaml = None
            for manifest in manifests:
                if manifest and manifest.get("kind") == "PersistentVolumeClaim":
                    # Auto-detect and set storage class
                    storage_class = self._get_available_storage_class()
                    if storage_class:
                        manifest["spec"]["storageClassName"] = storage_class
                    apply_manifest(self.k8s_client, manifest, f"{service_config['name']} PVC")

                elif manifest and manifest.get("kind") == "Deployment":
                    deployment_yaml = manifest

                    # Inject environment variables
                    if "env_vars" in service_config:
                        self.loader.inject_env_vars(deployment_yaml, service_config["env_vars"])

                    # Override image if specified
                    if "image_override" in service_config:
                        self.loader.inject_image_override(deployment_yaml, service_config["image_override"])

                    # Apply deployment
                    apply_manifest(self.k8s_client, deployment_yaml, f"{service_config['name']} Deployment")

                elif manifest and manifest.get("kind") == "Service":
                    # Apply service from YAML
                    apply_manifest(self.k8s_client, manifest, f"{service_config['name']} Service")

            # Create service if not in YAML and service_port is specified
            if deployment_yaml and "service_port" in service_config:
                # Check if service was already created from YAML
                has_service = any(m.get("kind") == "Service" for m in manifests if m)
                if not has_service:
                    service_manifest = self._create_service_manifest(
                        deployment_yaml["metadata"]["name"],
                        service_config["service_port"]
                    )
                    apply_manifest(self.k8s_client, service_manifest, f"{service_config['name']} Service")

        except Exception as e:
            console.print(f"[red]❌ Error deploying {service_config['name']}: {e}[/]")
            raise

    def _create_service_manifest(self, name, port, service_type="ClusterIP"):
        """Create a standard Kubernetes service manifest."""
        return {
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {"name": name, "namespace": NAMESPACE},
            "spec": {
                "type": service_type,
                "selector": {"app": name},
                "ports": [{"protocol": "TCP", "port": port, "targetPort": port}]
            }
        }

    def deploy_auth_service(self, **auth_config):
        """Deploy auth service from YAML without dynamic injection."""
        console.rule("[bold magenta]5. Auth Service[/]")

        try:
            file_path = self.loader.charts_dir / "services/auth-service/deployment.yaml"
            with open(file_path, 'r') as f:
                content = f.read()
            
            # Replace template variables
            template_vars = self.global_template_vars.copy()
            for key, value in template_vars.items():
                placeholder = f"{{{{ {key} }}}}"
                content = content.replace(placeholder, str(value))

            # Load and apply manifests
            for doc in yaml.safe_load_all(content):
                if doc:  # Skip empty documents
                    resource_name = doc.get('metadata', {}).get('name', 'Auth Service')
                    apply_manifest(self.k8s_client, doc, f"Auth Service: {resource_name}")

        except Exception as e:
            console.print(f"[yellow]⚠️  Warning: Could not deploy auth service from YAML: {e}[/]")
            console.print("[yellow]   Auth service is optional and can be deployed separately if needed[/]")

    def deploy_agent_gateway(self):
        """Deploy agent gateway using existing YAML files."""
        console.rule("[bold magenta]6. Agent Gateway (Kong + Service Registry)[/]")

        # Get LoadBalancer configuration for the cloud provider
        lb_config = self.get_or_create_static_ip()

        gateway_files = [
            {"path": "services/agent-gateway/kong-migrations.yaml", "desc": "Kong Migrations"},
            {"path": "services/agent-gateway/kong-plugins-config.yaml", "desc": "Kong Plugins Config"},
            {"path": "services/agent-gateway/deployment.yaml", "desc": "Kong Gateway + Chat History"},
            {"path": "services/agent-gateway/service-registry-deployment.yaml", "desc": "Service Registry"}
        ]

        for file_config in gateway_files:
            try:
                file_path = self.loader.charts_dir / file_config["path"]
                with open(file_path, 'r') as f:
                    content = f.read()

                # Replace template variables
                template_vars = self.global_template_vars.copy()
                for key, value in template_vars.items():
                    placeholder = f"{{{{ {key} }}}}"
                    content = content.replace(placeholder, str(value))

                # Load and apply manifests
                for doc in yaml.safe_load_all(content):
                    if doc:  # Skip empty documents
                        # Inject cloud-specific LoadBalancer config for kong-gateway service
                        if (doc.get("kind") == "Service" and
                            doc.get("metadata", {}).get("name") == "kong-gateway" and
                            lb_config):
                            self._inject_loadbalancer_config(doc, lb_config)

                        resource_name = doc.get('metadata', {}).get('name', file_config['desc'])
                        apply_manifest(self.k8s_client, doc, f"{file_config['desc']}: {resource_name}")
            except Exception as e:
                console.print(f"[yellow]⚠️  Warning: Could not deploy {file_config['desc']}: {e}[/]")

    def _inject_loadbalancer_config(self, service_manifest, lb_config):
        """Inject cloud-specific LoadBalancer configuration into kong-gateway service manifest."""
        provider = lb_config["provider"]

        # Ensure annotations dict exists
        if "annotations" not in service_manifest["metadata"]:
            service_manifest["metadata"]["annotations"] = {}

        annotations = service_manifest["metadata"]["annotations"]

        if provider == "digitalocean":
            # DigitalOcean LoadBalancer annotations - generate unique name
            import time
            import random
            timestamp = int(time.time())
            random_suffix = random.randint(1000, 9999)
            unique_name = f"nasiko-gateway-{timestamp}-{random_suffix}"
            annotations["service.beta.kubernetes.io/do-loadbalancer-name"] = unique_name
            annotations["service.beta.kubernetes.io/do-loadbalancer-size-unit"] = "1"
            console.print(f"[cyan]✅ Configured DigitalOcean LoadBalancer: {unique_name}[/]")

        elif provider == "aws":
            # AWS NLB annotations (works with in-tree controller)
            annotations["service.beta.kubernetes.io/aws-load-balancer-type"] = "nlb"
            annotations["service.beta.kubernetes.io/aws-load-balancer-scheme"] = "internet-facing"
            console.print("[cyan]✅ Configured AWS NLB LoadBalancer[/]")

    def get_kong_gateway_url(self, timeout=300, check_interval=10):
        """Wait for Kong gateway LoadBalancer to get external IP and return the URL."""
        import time

        console.print("[cyan]⏳ Waiting for Kong gateway LoadBalancer to get external IP...[/]")

        v1 = client.CoreV1Api(self.k8s_client)
        elapsed = 0

        while elapsed < timeout:
            try:
                service = v1.read_namespaced_service(name="kong-gateway", namespace=NAMESPACE)

                # Check for LoadBalancer ingress
                if service.status and service.status.load_balancer and service.status.load_balancer.ingress:
                    ingress = service.status.load_balancer.ingress[0]

                    # Handle both object (attribute access) and dict (dictionary access)
                    external_ip = None
                    if hasattr(ingress, 'ip'):
                        external_ip = ingress.ip or getattr(ingress, 'hostname', None)
                    elif isinstance(ingress, dict):
                        external_ip = ingress.get('ip') or ingress.get('hostname')

                    if external_ip:
                        # Construct the gateway URL (assuming HTTP on port 80)
                        gateway_url = f"http://{external_ip}"
                        console.print(f"[green]✅ Kong gateway is available at: {gateway_url}[/]")
                        self.gateway_url = gateway_url
                        return gateway_url

                # Still pending
                console.print(f"[dim]   Still waiting for LoadBalancer IP... ({elapsed}s/{timeout}s)[/]")
                time.sleep(check_interval)
                elapsed += check_interval

            except Exception as e:
                console.print(f"[yellow]⚠️  Error checking service: {e}[/]")
                time.sleep(check_interval)
                elapsed += check_interval

        console.print(f"[yellow]⚠️  Timeout waiting for Kong gateway LoadBalancer IP after {timeout}s[/]")
        console.print(f"[yellow]   You may need to manually set GATEWAY_URL environment variable[/]")
        return None

    def deploy_router_service(self):
        """Deploy nasiko-router service from YAML with dynamic injection."""
        console.rule("[bold magenta]7. Router Service[/]")

        try:
            # Read and apply template variables to the YAML content
            router_yaml_path = self.loader.charts_dir / "services" / "nasiko-router" / "deployment.yaml"
            with open(router_yaml_path, 'r') as f:
                content = f.read()

            # Replace template variables
            template_vars = self.global_template_vars.copy()
            for key, value in template_vars.items():
                placeholder = f"{{{{ {key} }}}}"
                content = content.replace(placeholder, str(value))

            # Load all manifests
            router_manifests = list(yaml.safe_load_all(content))

            for manifest in router_manifests:
                if manifest:
                    apply_manifest(self.k8s_client, manifest, f"Router {manifest['kind']}")

        except Exception as e:
            console.print(f"[red]❌ Error deploying Router service: {e}[/]")
            raise

    def deploy_superuser_init(self, username: str = "admin", email: str = "admin@nasiko.com"):
        """Deploy superuser initialization job."""
        console.rule("[bold magenta]8. Super User Initialization[/]")

        try:
            file_path = self.loader.charts_dir / "initialization" / "superuser-init.yaml"
            with open(file_path, 'r') as f:
                content = f.read()

            # Replace template variables
            content = content.replace("{{ SUPERUSER_USERNAME }}", username)
            content = content.replace("{{ SUPERUSER_EMAIL }}", email)

            # Load and apply all manifests (file contains ConfigMap + Job)
            manifests = list(yaml.safe_load_all(content))
            for manifest in manifests:
                if manifest:
                    # Determine resource type for better logging
                    kind = manifest.get('kind', 'Resource')
                    name = manifest.get('metadata', {}).get('name', 'unknown')
                    apply_manifest(self.k8s_client, manifest, f"{kind}: {name}")

            console.print(f"[green]✅ Super user initialization job created[/]")
            console.print(f"[cyan]   Username: {username}[/]")
            console.print(f"[cyan]   Email: {email}[/]")
            console.print(f"[dim]   Job will create user and store credentials in 'superuser-credentials' secret[/]")

        except Exception as e:
            console.print(f"[yellow]⚠️  Warning: Could not deploy superuser init job: {e}[/]")
            console.print(f"[yellow]   You can create the super user manually after deployment[/]")


# --- Main Deploy Command ---

@app.command()
def deploy(
    registry_url: str = typer.Option(..., help="Registry URL (Harbor Domain or Cloud URI)"),
    registry_user: str = typer.Option(None, help="Registry User"),
    registry_pass: str = typer.Option(None, help="Registry Pass"),
    public_user: str = typer.Option(DEFAULT_PUBLIC_REGISTRY_USER, help="Docker Hub user for public images"),
    openai_key: str = typer.Option(None, help="OpenAI API Key"),
    environment: str = typer.Option("default", help="Environment (dev/prod/default)"),
    superuser_username: str = typer.Option("admin", help="Super user username"),
    superuser_email: str = typer.Option("admin@nasiko.com", help="Super user email"),
    provider: str = typer.Option(None, help="Cloud provider (aws, digitalocean) for LoadBalancer config"),
    region: str = typer.Option(None, help="Cloud region for provider-specific settings")
):
    """Deploy Nasiko using YAML manifests + Python secrets approach."""

    # Setup registry configuration
    registry_config = {
        "url": registry_url.replace("https://", "").replace("http://", ""),
        "public": f"docker.io/{public_user}"
    }

    # Connect to Kubernetes
    kube_config_path = os.environ.get("KUBECONFIG")
    if not kube_config_path:
        console.print("[red]❌ KUBECONFIG environment variable is not set[/]")
        console.print("\n[yellow]Please set KUBECONFIG to point to your cluster config:[/]")
        console.print("[cyan]  export KUBECONFIG=/path/to/your/kubeconfig.yaml[/]")
        sys.exit(1)

    try:
        console.print(f"[dim]Loading kubeconfig from: {kube_config_path}[/]")
        config.load_kube_config(config_file=kube_config_path)
        k8s_client = client.ApiClient()
        console.print("[green]✅ Connected to Kubernetes[/]")
    except Exception as e:
        console.print(f"[red]❌ Failed to load kubeconfig: {str(e)}[/]")
        console.print(f"\n[yellow]Current KUBECONFIG: {kube_config_path}[/]")
        if not os.path.exists(kube_config_path):
            console.print(f"[yellow]⚠️  File not found at: {kube_config_path}[/]")
        sys.exit(1)

    # Initialize deployer
    deployer = NasikoDeployer(k8s_client, registry_config, environment, provider, region)

    # Execute deployment phases
    try:
        # 0. Namespaces (MUST be first!)
        deployer.deploy_namespaces()

        # 1. RBAC
        deployer.deploy_rbac()

        # Prepare template variables for all deployments
        mongo_user = os.getenv("MONGO_NASIKO_USER", "root")
        # Use a URL-safe password so it works in Mongo URIs without special-casing.
        mongo_password = os.getenv("MONGO_NASIKO_PASSWORD") or ("password" + base64.b64encode(os.urandom(6)).decode('utf-8'))
        mongo_url = (
            "mongodb://"
            f"{urllib.parse.quote(mongo_user, safe='')}"
            f":{urllib.parse.quote(mongo_password, safe='')}"
            "@mongodb:27017"
        )
        template_vars = {
            "PUBLIC_REGISTRY": registry_config["public"],
            "MONGO_NASIKO_USER": mongo_user,
            "MONGO_NASIKO_PASSWORD": mongo_password,
            "MONGO_URL": mongo_url,
            "JWT_SECRET": os.getenv("JWT_SECRET") or base64.b64encode(os.urandom(32)).decode('utf-8'),
            "OPENAI_API_KEY": openai_key or "",
            "OPENROUTER_API_KEY": os.getenv("OPENROUTER_API_KEY", ""),
            "GITHUB_CLIENT_ID": os.getenv("GITHUB_CLIENT_ID", ""),
            "GITHUB_CLIENT_SECRET": os.getenv("GITHUB_CLIENT_SECRET", ""),
            "USER_CREDENTIALS_ENCRYPTION_KEY": os.getenv("USER_CREDENTIALS_ENCRYPTION_KEY") or base64.b64encode(os.urandom(32)).decode('utf-8'),
        }

        # 2. Infrastructure
        deployer.deploy_infrastructure(**template_vars)

        # 3. Secrets
        secret_vars = {
            "openai_api_key": template_vars["OPENAI_API_KEY"],
            "jwt_secret": template_vars["JWT_SECRET"],
            "mongo_password": template_vars["MONGO_NASIKO_PASSWORD"],
            "encryption_key": template_vars["USER_CREDENTIALS_ENCRYPTION_KEY"],
        }
        
        # Add DO token if available
        do_token = (
            os.getenv("DIGITALOCEAN_ACCESS_TOKEN")
            or os.getenv("DO_TOKEN")
            or os.getenv("TF_VAR_do_token")
        )
        # Add DO token if available
        do_token = (
            os.getenv("DIGITALOCEAN_ACCESS_TOKEN")
            or os.getenv("DO_TOKEN")
            or os.getenv("TF_VAR_do_token")
        )
        if do_token:
            secret_vars["do_token"] = do_token

        deployer.deploy_secrets(
            registry_user=registry_user,
            registry_pass=registry_pass,
            **secret_vars
        )

        # Update deployer's service deployment logic to use these template_vars
        deployer.global_template_vars = template_vars

        # 4. Core services
        deployer.deploy_core_services()

        # 5. Auth service
        deployer.deploy_auth_service()

        # 6. Agent gateway
        deployer.deploy_agent_gateway()

        # 6.5. Wait for gateway LoadBalancer IP and retrieve gateway URL
        gateway_url = deployer.get_kong_gateway_url()
        
        if gateway_url:
            # Save gateway URL to cluster state and active context
            cluster_name = os.environ.get("NASIKO_CLUSTER_NAME", "default-cluster")
            save_cluster_info(provider, cluster_name, {"gateway_url": gateway_url})

        # 6.75. K8s Build Worker (deployed AFTER gateway to use gateway URL)
        deployer.deploy_k8s_build_worker()

        # 6.8. N8N Automation Service (deployed AFTER gateway to use gateway URL)
        deployer.deploy_n8n()

        # 6.8. N8N Automation Service (deployed AFTER gateway to use gateway URL)
        deployer.deploy_n8n()

        # 7. Router service
        deployer.deploy_router_service()

        # 8. Super user initialization
        deployer.deploy_superuser_init(username=superuser_username, email=superuser_email)

        console.print("\n[bold green]✅ Full Nasiko Stack Deployed Successfully![/]")
        console.print("\n[bold cyan]Next steps:[/]")
        console.print("• Wait for all pods to be ready: [cyan]kubectl get pods -n nasiko[/]")
        console.print("• Check services: [cyan]kubectl get services -n nasiko[/]")
        console.print("• View worker logs: [cyan]kubectl logs -n nasiko -l app=nasiko-k8s-build-worker -f[/]")
        console.print("• Super user job: [cyan]kubectl get jobs -n nasiko superuser-init[/]")

    except KeyboardInterrupt:
        console.print("\n[yellow]⚠️  Deployment interrupted by user[/]")
        sys.exit(1)
    except Exception as e:
        console.print(f"\n[red]❌ Deployment failed: {e}[/]")
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    app()
