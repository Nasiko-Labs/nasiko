"""
Configuration management for Nasiko CLI setup commands.

Supports loading configuration from:
1. Environment files (.env, .nasiko.env, .nasiko-aws.env, .nasiko-do.env)
2. Environment variables
3. CLI arguments (highest priority)

Priority order (highest to lowest):
1. CLI arguments (explicit user input)
2. Environment variables (including from loaded .env file)
3. Built-in defaults

Terraform State Management:
- By default, state is stored locally in ~/.nasiko/state/<provider>/<cluster-name>/
- Remote backends (S3, Terraform Cloud) can be configured via environment variables
"""

import os
import json
from pathlib import Path
from typing import Optional, List, Dict, Any
from rich.console import Console

console = Console()


def _ensure_dir_permissions(path: Path, mode: int = 0o755):
    """
    Ensure a directory and its parents have proper permissions.

    Args:
        path: Directory path to fix
        mode: Permission mode (default: 0o755 = rwxr-xr-x)
    """
    path.chmod(mode)
    # Fix up to 2 levels of parents (state_root and provider_dir)
    for parent in [path.parent, path.parent.parent]:
        if parent.exists() and parent != Path.home():
            try:
                parent.chmod(mode)
            except (OSError, PermissionError):
                pass  # Skip if we can't fix parent permissions


# =============================================================================
# Global Constants - Change these in ONE place to update across the CLI
# =============================================================================

# Default Docker Hub user for pulling pre-built Nasiko core application images
# (nasiko-backend, nasiko-web, nasiko-auth, nasiko-router, etc.)
DEFAULT_PUBLIC_REGISTRY_USER = "akhilfolium"

# =============================================================================

# Environment variable mappings
# Maps CLI parameter names to environment variable names
ENV_VAR_MAPPING = {
    # Cluster configuration
    "provider": "NASIKO_PROVIDER",
    "region": "NASIKO_REGION",
    "cluster_name": "NASIKO_CLUSTER_NAME",
    "kubeconfig": "KUBECONFIG",

    # Registry configuration
    "registry_type": "NASIKO_CONTAINER_REGISTRY_TYPE",
    "cloud_reg_name": "NASIKO_CONTAINER_REGISTRY_NAME",
    "registry_user": "NASIKO_REGISTRY_USER",
    "registry_pass": "NASIKO_REGISTRY_PASS",
    "domain": "NASIKO_DOMAIN",
    "email": "NASIKO_EMAIL",

    # Application configuration
    "openai_key": "OPENAI_API_KEY",
    "public_registry_user": "NASIKO_PUBLIC_REGISTRY_USER",

    # Super user configuration
    "superuser_username": "NASIKO_SUPERUSER_USERNAME",
    "superuser_email": "NASIKO_SUPERUSER_EMAIL",

    # GitHub OAuth (optional)
    "github_client_id": "GITHUB_CLIENT_ID",
    "github_client_secret": "GITHUB_CLIENT_SECRET",

    # Terraform configuration
    "terraform_dir": "NASIKO_TERRAFORM_DIR",  # Path to terraform modules
    "state_dir": "NASIKO_STATE_DIR",          # Path for local state files

    # Remote state backend configuration (S3)
    "tf_backend": "NASIKO_TF_BACKEND",                  # Backend type: local, s3, gcs, remote
    "tf_backend_bucket": "NASIKO_TF_BACKEND_BUCKET",    # S3/GCS bucket name
    "tf_backend_region": "NASIKO_TF_BACKEND_REGION",    # S3 bucket region
    "tf_backend_key_prefix": "NASIKO_TF_BACKEND_KEY_PREFIX",  # State file path prefix
    "tf_backend_dynamodb_table": "NASIKO_TF_BACKEND_DYNAMODB_TABLE",  # DynamoDB table for locking

    # Terraform Cloud configuration
    "tf_cloud_org": "NASIKO_TF_CLOUD_ORG",              # Terraform Cloud organization
    "tf_cloud_workspace": "NASIKO_TF_CLOUD_WORKSPACE",  # Terraform Cloud workspace prefix

    # Database Configuration
    
}

# Default config file search paths (in order of priority)
CONFIG_FILE_SEARCH_PATHS = [
    ".nasiko-local.env",
    ".nasiko.env",           # Project-specific
    ".nasiko-aws.env",       # AWS-specific
    ".nasiko-do.env",        # DigitalOcean-specific
    ".env",                  # Generic
]


def find_config_file(config_path: Optional[str] = None) -> Optional[Path]:
    """
    Find a configuration file to load.

    Args:
        config_path: Optional explicit path to config file

    Returns:
        Path to config file if found, None otherwise
    """
    # If explicit path provided, use it
    if config_path:
        path = Path(config_path).resolve()
        if path.exists():
            return path
        else:
            console.print(f"[yellow]Warning: Config file not found: {path}[/]")
            return None

    # Search for config files in current directory
    cwd = Path.cwd()
    for filename in CONFIG_FILE_SEARCH_PATHS:
        path = cwd / filename
        if path.exists():
            return path

    return None


def load_config_file(config_path: Optional[str] = None, verbose: bool = True) -> bool:
    """
    Load configuration from an environment file.

    This function loads environment variables from a .env file into os.environ.
    Existing environment variables are NOT overwritten (CLI args and existing
    env vars take precedence).

    Args:
        config_path: Optional explicit path to config file
        verbose: Whether to print status messages

    Returns:
        True if a config file was loaded, False otherwise
    """
    try:
        from dotenv import load_dotenv
    except ImportError:
        if verbose:
            console.print("[yellow]Warning: python-dotenv not installed, skipping .env file loading[/]")
        return False

    config_file = find_config_file(config_path)

    if config_file:
        # Load the env file, but don't override existing env vars
        load_dotenv(config_file, override=False)
        if verbose:
            console.print(f"[dim]Loaded configuration from: {config_file}[/]")
        return True

    return False


def get_env_var(param_name: str, default: Optional[str] = None) -> Optional[str]:
    """
    Get environment variable value for a parameter.

    Args:
        param_name: Parameter name (e.g., 'provider', 'region')
        default: Default value if not found

    Returns:
        Environment variable value or default
    """
    env_var = ENV_VAR_MAPPING.get(param_name)
    if env_var:
        return os.environ.get(env_var, default)
    return default


def print_config_summary(loaded_from_file: bool, config_file: Optional[Path] = None):
    """Print a summary of loaded configuration."""
    console.print("\n[bold cyan]Configuration Summary:[/]")

    if loaded_from_file and config_file:
        console.print(f"  Config file: {config_file}")

    # Show which key environment variables are set
    key_vars = [
        ("NASIKO_PROVIDER", "Provider"),
        ("NASIKO_REGION", "Region"),
        ("NASIKO_CLUSTER_NAME", "Cluster Name"),
        ("KUBECONFIG", "Kubeconfig"),
        ("NASIKO_CONTAINER_REGISTRY_TYPE", "Registry Type"),
        ("NASIKO_CONTAINER_REGISTRY_NAME", "Registry Name"),
        ("OPENAI_API_KEY", "OpenAI Key"),
        ("DIGITALOCEAN_ACCESS_TOKEN", "DigitalOcean Token"),
        ("DO_TOKEN", "DigitalOcean Token"),
        ("AWS_ACCESS_KEY_ID", "AWS Access Key"),
    ]

    for env_var, display_name in key_vars:
        value = os.environ.get(env_var)
        if value:
            # Mask sensitive values
            if "KEY" in env_var or "SECRET" in env_var or "TOKEN" in env_var or "PASS" in env_var:
                masked = value[:4] + "..." + value[-4:] if len(value) > 8 else "****"
                console.print(f"  {display_name}: [dim]{masked}[/]")
            else:
                console.print(f"  {display_name}: [cyan]{value}[/]")

    console.print()


def validate_required_credentials(provider: Optional[str] = None) -> List[str]:
    """
    Validate that required credentials are available.

    Args:
        provider: Cloud provider (aws, digitalocean, or None)

    Returns:
        List of missing credential names
    """
    missing = []

    if provider == "aws":
        if not os.environ.get("AWS_ACCESS_KEY_ID"):
            missing.append("AWS_ACCESS_KEY_ID")
        if not os.environ.get("AWS_SECRET_ACCESS_KEY"):
            missing.append("AWS_SECRET_ACCESS_KEY")
    elif provider == "digitalocean":
        if not (
            os.environ.get("DIGITALOCEAN_ACCESS_TOKEN")
            or os.environ.get("DO_TOKEN")
            or os.environ.get("TF_VAR_do_token")
        ):
            missing.append("DIGITALOCEAN_ACCESS_TOKEN")

    return missing


# --- Terraform and State Directory Functions ---

def get_nasiko_home() -> Path:
    """
    Returns the Nasiko home directory: ~/.nasiko
    Creates it if it doesn't exist, with proper permissions.
    """
    nasiko_home = Path.home() / ".nasiko"
    nasiko_home.mkdir(parents=True, exist_ok=True)
    nasiko_home.chmod(0o755)
    return nasiko_home


def get_default_terraform_dir() -> Path:
    """
    Returns the default Terraform modules directory: ~/.nasiko/terraform
    This is where bundled or downloaded Terraform modules are stored.
    """
    tf_dir = get_nasiko_home() / "terraform"
    tf_dir.mkdir(parents=True, exist_ok=True)
    tf_dir.chmod(0o755)
    return tf_dir


def get_terraform_dir(cli_override: Optional[str] = None) -> Path:
    """
    Get the Terraform modules directory with priority:
    1. CLI argument override
    2. NASIKO_TERRAFORM_DIR environment variable
    3. Default: ~/.nasiko/terraform

    Args:
        cli_override: Path specified via CLI argument

    Returns:
        Path to terraform modules directory
    """
    if cli_override:
        path = Path(cli_override).resolve()
        if path.exists():
            return path
        else:
            console.print(f"[yellow]Warning: Terraform directory not found: {path}[/]")
            console.print(f"[yellow]Falling back to default location[/]")

    env_path = os.environ.get("NASIKO_TERRAFORM_DIR")
    if env_path:
        path = Path(env_path).resolve()
        if path.exists():
            return path
        else:
            console.print(f"[yellow]Warning: NASIKO_TERRAFORM_DIR not found: {path}[/]")

    return get_default_terraform_dir()


def get_state_dir(provider: str, cluster_name: str, cli_override: Optional[str] = None) -> Path:
    """
    Get the Terraform state directory for a specific cluster.

    State is stored in: <state_root>/<provider>/<cluster_name>/

    Priority for state_root:
    1. CLI argument override
    2. NASIKO_STATE_DIR environment variable
    3. Default: ~/.nasiko/state

    Args:
        provider: Cloud provider (aws, digitalocean)
        cluster_name: Name of the cluster
        cli_override: Path specified via CLI argument

    Returns:
        Path to state directory for this cluster
    """
    # Determine state root directory
    if cli_override:
        state_root = Path(cli_override).resolve()
    elif os.environ.get("NASIKO_STATE_DIR"):
        state_root = Path(os.environ.get("NASIKO_STATE_DIR")).resolve()
    else:
        state_root = get_nasiko_home() / "state"

    # Create cluster-specific state directory with proper permissions
    state_dir = state_root / provider / cluster_name
    state_dir.mkdir(parents=True, exist_ok=True)
    _ensure_dir_permissions(state_dir)
    return state_dir


def get_backend_config() -> dict:
    """
    Get remote backend configuration from environment variables.

    Returns a dict with backend settings, or empty dict for local state.

    Supported backends:
    - local: (default) Store state in ~/.nasiko/state/
    - s3: AWS S3 with optional DynamoDB locking
    - gcs: Google Cloud Storage
    - remote: Terraform Cloud

    Returns:
        Dict with backend configuration
    """
    backend_type = os.environ.get("NASIKO_TF_BACKEND", "local").lower()

    if backend_type == "local":
        return {"type": "local"}

    elif backend_type == "s3":
        config = {
            "type": "s3",
            "bucket": os.environ.get("NASIKO_TF_BACKEND_BUCKET"),
            "region": os.environ.get("NASIKO_TF_BACKEND_REGION", "us-east-1"),
            "key_prefix": os.environ.get("NASIKO_TF_BACKEND_KEY_PREFIX", "nasiko/terraform"),
        }
        # Optional DynamoDB table for state locking
        dynamodb_table = os.environ.get("NASIKO_TF_BACKEND_DYNAMODB_TABLE")
        if dynamodb_table:
            config["dynamodb_table"] = dynamodb_table

        if not config["bucket"]:
            console.print("[yellow]Warning: NASIKO_TF_BACKEND=s3 but NASIKO_TF_BACKEND_BUCKET not set[/]")
            console.print("[yellow]Falling back to local state[/]")
            return {"type": "local"}

        return config

    elif backend_type == "gcs":
        config = {
            "type": "gcs",
            "bucket": os.environ.get("NASIKO_TF_BACKEND_BUCKET"),
            "prefix": os.environ.get("NASIKO_TF_BACKEND_KEY_PREFIX", "nasiko/terraform"),
        }
        if not config["bucket"]:
            console.print("[yellow]Warning: NASIKO_TF_BACKEND=gcs but NASIKO_TF_BACKEND_BUCKET not set[/]")
            return {"type": "local"}
        return config

    elif backend_type == "remote":
        # Terraform Cloud / Terraform Enterprise
        config = {
            "type": "remote",
            "organization": os.environ.get("NASIKO_TF_CLOUD_ORG"),
            "workspace_prefix": os.environ.get("NASIKO_TF_CLOUD_WORKSPACE", "nasiko-"),
        }
        if not config["organization"]:
            console.print("[yellow]Warning: NASIKO_TF_BACKEND=remote but NASIKO_TF_CLOUD_ORG not set[/]")
            return {"type": "local"}
        return config

    else:
        console.print(f"[yellow]Warning: Unknown backend type '{backend_type}', using local[/]")
        return {"type": "local"}


def get_cluster_credentials_file(cluster_name: str, provider: Optional[str] = None) -> Path:
    """
    Get the path to the superuser credentials file for a specific cluster.

    Credentials are stored in: ~/.nasiko/state/<provider>/<cluster_name>/superuser-credentials.json
    For existing clusters (no provider), stored in: ~/.nasiko/state/existing/<cluster_name>/superuser-credentials.json

    Args:
        cluster_name: Name of the cluster
        provider: Cloud provider (aws, digitalocean) or None for existing clusters

    Returns:
        Path to credentials file
    """
    state_root = get_nasiko_home() / "state"
    provider_dir = provider if provider else "existing"
    creds_dir = state_root / provider_dir / cluster_name
    creds_dir.mkdir(parents=True, exist_ok=True)
    _ensure_dir_permissions(creds_dir)
    return creds_dir / "superuser-credentials.json"


def get_cluster_info_file(cluster_name: str, provider: Optional[str] = None) -> Path:
    """
    Get the path to the cluster info file (API URLs, etc.) for a specific cluster.

    Stored in: ~/.nasiko/state/<provider>/<cluster_name>/cluster-info.json

    Args:
        cluster_name: Name of the cluster
        provider: Cloud provider (aws, digitalocean) or None

    Returns:
        Path to cluster info file
    """
    state_root = get_nasiko_home() / "state"
    provider_dir = provider if provider else "existing"
    info_dir = state_root / provider_dir / cluster_name
    info_dir.mkdir(parents=True, exist_ok=True)
    _ensure_dir_permissions(info_dir)
    return info_dir / "cluster-info.json"


def save_cluster_info(provider: Optional[str], cluster_name: str, data: Dict[str, Any]):
    """
    Save cluster information (URLs, metadata) to the state directory.
    
    Args:
        provider: Cloud provider
        cluster_name: Cluster name
        data: Dictionary containing cluster info
    """
    info_file = get_cluster_info_file(cluster_name, provider)
    
    try:
        # Update existing if present
        if info_file.exists():
            with open(info_file, 'r') as f:
                existing = json.load(f)
            existing.update(data)
            data = existing
            
        with open(info_file, 'w') as f:
            json.dump(data, f, indent=2)
        info_file.chmod(0o644)  # rw-r--r--
        console.print(f"[dim]Saved cluster info to: {info_file}[/]")
    except Exception as e:
        console.print(f"[yellow]Warning: Failed to save cluster info: {e}[/]")


def get_cluster_api_url(cluster_name: str) -> Optional[str]:
    """
    Get the API URL for a specific cluster by checking state directories.
    
    Args:
        cluster_name: Name of the cluster
        
    Returns:
        API URL if found, None otherwise
    """
    # Search in all provider directories
    state_root = get_nasiko_home() / "state"
    if not state_root.exists():
        return None
        
    for provider_dir in state_root.iterdir():
        if not provider_dir.is_dir():
            continue
            
        cluster_dir = provider_dir / cluster_name
        info_file = cluster_dir / "cluster-info.json"
        
        if info_file.exists():
            try:
                with open(info_file, 'r') as f:
                    data = json.load(f)
                    return data.get("gateway_url")
            except Exception:
                continue
                
    return None

def list_clusters() -> List[Dict[str, Any]]:
    """
    List all configured clusters found in local state.
    
    Returns:
        List of dictionaries containing cluster info (name, provider, url, etc.)
    """
    clusters = []
    state_root = get_nasiko_home() / "state"
    
    if not state_root.exists():
        return clusters
        
    for provider_dir in state_root.iterdir():
        if not provider_dir.is_dir():
            continue
            
        for cluster_dir in provider_dir.iterdir():
            if not cluster_dir.is_dir():
                continue
                
            info_file = cluster_dir / "cluster-info.json"
            cluster_info = {
                "name": cluster_dir.name,
                "provider": provider_dir.name,
                "url": "Unknown",
                "path": str(cluster_dir)
            }
            
            if info_file.exists():
                try:
                    with open(info_file, 'r') as f:
                        data = json.load(f)
                        cluster_info.update(data)
                        if "gateway_url" in data:
                            cluster_info["url"] = data["gateway_url"]
                except Exception:
                    pass
            
            clusters.append(cluster_info)
            
    return clusters


def print_state_info(provider: str, cluster_name: str):
    """Print information about where Terraform state is stored."""
    backend = get_backend_config()

    console.print("\n[bold cyan]Terraform State Configuration:[/]")

    if backend["type"] == "local":
        state_dir = get_state_dir(provider, cluster_name)
        console.print(f"  Backend: [cyan]local[/]")
        console.print(f"  State Directory: [cyan]{state_dir}[/]")
        console.print(f"  [dim]Tip: Back up this directory to preserve your infrastructure state[/]")

    elif backend["type"] == "s3":
        key = f"{backend['key_prefix']}/{provider}/{cluster_name}/terraform.tfstate"
        console.print(f"  Backend: [cyan]s3[/]")
        console.print(f"  Bucket: [cyan]{backend['bucket']}[/]")
        console.print(f"  Key: [cyan]{key}[/]")
        console.print(f"  Region: [cyan]{backend['region']}[/]")
        if backend.get("dynamodb_table"):
            console.print(f"  Lock Table: [cyan]{backend['dynamodb_table']}[/]")

    elif backend["type"] == "gcs":
        console.print(f"  Backend: [cyan]gcs[/]")
        console.print(f"  Bucket: [cyan]{backend['bucket']}[/]")
        console.print(f"  Prefix: [cyan]{backend['prefix']}/{provider}/{cluster_name}[/]")

    elif backend["type"] == "remote":
        workspace = f"{backend['workspace_prefix']}{provider}-{cluster_name}"
        console.print(f"  Backend: [cyan]Terraform Cloud[/]")
        console.print(f"  Organization: [cyan]{backend['organization']}[/]")
        console.print(f"  Workspace: [cyan]{workspace}[/]")

    console.print()
