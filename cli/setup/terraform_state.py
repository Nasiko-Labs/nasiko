"""
Terraform State Management for Nasiko CLI.

This module handles:
1. Setting up working directories for Terraform operations
2. Generating backend configuration for state storage
3. Copying Terraform modules to working directories
4. Managing state file locations

Architecture:
- Terraform modules: ~/.nasiko/terraform/<provider>/  (or custom NASIKO_TERRAFORM_DIR)
- Working directory: ~/.nasiko/state/<provider>/<cluster-name>/
- Each cluster gets its own working directory with a copy of the modules
- State files are stored in the working directory (local backend) or remote

For remote backends (S3, GCS, Terraform Cloud):
- Backend configuration is generated dynamically as backend.tf
- State is stored in the remote backend with cluster-specific keys
"""

import os
import shutil
from pathlib import Path
from typing import Optional
from rich.console import Console

from .config import (
    get_terraform_dir,
    get_state_dir,
    get_backend_config,
)

console = Console()


def setup_working_directory(
    provider: str,
    cluster_name: str,
    terraform_dir: Optional[str] = None,
    state_dir: Optional[str] = None,
) -> Path:
    """
    Set up a Terraform working directory for a specific cluster.

    This function:
    1. Creates a working directory in the state location
    2. Copies Terraform modules to the working directory
    3. Generates backend configuration

    Args:
        provider: Cloud provider (aws, digitalocean)
        cluster_name: Name of the cluster
        terraform_dir: Override path to terraform modules
        state_dir: Override path for state storage

    Returns:
        Path to the working directory (where terraform commands should run)
    """
    # Get the terraform modules source directory
    tf_source = get_terraform_dir(terraform_dir)
    provider_source = tf_source / provider

    if not provider_source.exists():
        console.print(f"[bold red]Terraform modules not found for provider '{provider}'[/]")
        console.print(f"[red]Expected location: {provider_source}[/]")
        console.print("\n[yellow]To fix this, either:[/]")
        console.print(f"  1. Copy Terraform modules to: {tf_source}")
        console.print("  2. Set NASIKO_TERRAFORM_DIR to your modules location")
        console.print("  3. Use --terraform-dir CLI option")
        raise FileNotFoundError(f"Terraform modules not found: {provider_source}")

    # Get or create the working directory
    work_dir = get_state_dir(provider, cluster_name, state_dir)

    # Check if modules are already in working directory
    main_tf = work_dir / "main.tf"
    if not main_tf.exists():
        console.print(f"[dim]Copying Terraform modules to working directory...[/]")
        _copy_terraform_modules(provider_source, work_dir)

    # Generate backend configuration
    _generate_backend_config(work_dir, provider, cluster_name)

    console.print(f"[dim]Working directory: {work_dir}[/]")
    return work_dir


def _copy_terraform_modules(source: Path, dest: Path):
    """
    Copy Terraform module files to the working directory.

    Only copies .tf files and required supporting files.
    Does NOT copy .terraform/ directory or state files.
    """
    # Files to copy
    tf_extensions = {".tf", ".tfvars"}
    exclude_patterns = {".terraform", "terraform.tfstate", ".tfstate"}

    for item in source.iterdir():
        # Skip excluded patterns
        if any(excl in item.name for excl in exclude_patterns):
            continue

        dest_path = dest / item.name

        if item.is_file() and item.suffix in tf_extensions:
            shutil.copy2(item, dest_path)
        elif item.is_file() and item.name in {"versions.tf", "providers.tf", "outputs.tf", "variables.tf"}:
            shutil.copy2(item, dest_path)


def _generate_backend_config(work_dir: Path, provider: str, cluster_name: str):
    """
    Generate a backend.tf file for Terraform state configuration.

    The backend configuration is generated based on environment variables
    or defaults to local state storage.
    """
    backend_config = get_backend_config()
    backend_file = work_dir / "backend.tf"

    # Remove existing backend.tf to avoid conflicts
    if backend_file.exists():
        backend_file.unlink()

    backend_type = backend_config.get("type", "local")

    if backend_type == "local":
        # Local backend - state stored in working directory
        # No explicit backend config needed, terraform uses local by default
        content = '''# Backend Configuration - Local State
# State file: terraform.tfstate (in this directory)
#
# To migrate to remote state, set these environment variables:
#   NASIKO_TF_BACKEND=s3
#   NASIKO_TF_BACKEND_BUCKET=your-bucket-name
#   NASIKO_TF_BACKEND_REGION=us-east-1
#   NASIKO_TF_BACKEND_DYNAMODB_TABLE=terraform-locks (optional, for locking)
#
# Then run: nasiko setup k8s create <provider> (terraform will prompt for migration)

terraform {
  backend "local" {
    # State is stored in terraform.tfstate in this directory
  }
}
'''

    elif backend_type == "s3":
        bucket = backend_config["bucket"]
        region = backend_config["region"]
        key = f"{backend_config['key_prefix']}/{provider}/{cluster_name}/terraform.tfstate"
        dynamodb = backend_config.get("dynamodb_table")

        content = f'''# Backend Configuration - AWS S3
# State stored in: s3://{bucket}/{key}

terraform {{
  backend "s3" {{
    bucket = "{bucket}"
    key    = "{key}"
    region = "{region}"
'''
        if dynamodb:
            content += f'    dynamodb_table = "{dynamodb}"\n'
        content += '''    encrypt = true
  }
}
'''

    elif backend_type == "gcs":
        bucket = backend_config["bucket"]
        prefix = f"{backend_config['prefix']}/{provider}/{cluster_name}"

        content = f'''# Backend Configuration - Google Cloud Storage
# State stored in: gs://{bucket}/{prefix}/default.tfstate

terraform {{
  backend "gcs" {{
    bucket = "{bucket}"
    prefix = "{prefix}"
  }}
}}
'''

    elif backend_type == "remote":
        org = backend_config["organization"]
        workspace = f"{backend_config['workspace_prefix']}{provider}-{cluster_name}"

        content = f'''# Backend Configuration - Terraform Cloud
# Organization: {org}
# Workspace: {workspace}

terraform {{
  cloud {{
    organization = "{org}"

    workspaces {{
      name = "{workspace}"
    }}
  }}
}}
'''

    else:
        # Fallback to local
        content = "# Using default local backend\n"

    backend_file.write_text(content)


def get_cluster_state_info(provider: str, cluster_name: str, state_dir: Optional[str] = None) -> dict:
    """
    Get information about the state of a specific cluster.

    Returns:
        Dict with state information including:
        - exists: Whether state exists for this cluster
        - work_dir: Path to working directory
        - backend_type: Type of backend (local, s3, etc.)
        - state_file: Path to state file (for local backend)
    """
    work_dir = get_state_dir(provider, cluster_name, state_dir)
    backend_config = get_backend_config()

    info = {
        "exists": False,
        "work_dir": work_dir,
        "backend_type": backend_config.get("type", "local"),
        "state_file": None,
        "has_modules": False,
    }

    # Check if working directory has modules
    if (work_dir / "main.tf").exists() or (work_dir / "doks.tf").exists():
        info["has_modules"] = True

    # Check for local state
    state_file = work_dir / "terraform.tfstate"
    if state_file.exists():
        info["exists"] = True
        info["state_file"] = state_file

    return info


def list_managed_clusters(state_root: Optional[str] = None) -> list:
    """
    List all clusters that have Terraform state managed by Nasiko.

    Returns:
        List of dicts with cluster info: {provider, cluster_name, work_dir}
    """
    if state_root:
        root = Path(state_root).resolve()
    elif os.environ.get("NASIKO_STATE_DIR"):
        root = Path(os.environ.get("NASIKO_STATE_DIR")).resolve()
    else:
        from .config import get_nasiko_home
        root = get_nasiko_home() / "state"

    clusters = []

    if not root.exists():
        return clusters

    for provider_dir in root.iterdir():
        if not provider_dir.is_dir():
            continue
        provider = provider_dir.name

        for cluster_dir in provider_dir.iterdir():
            if not cluster_dir.is_dir():
                continue
            cluster_name = cluster_dir.name

            # Check if this looks like a terraform working directory
            has_tf = (cluster_dir / "terraform.tfstate").exists() or \
                     (cluster_dir / ".terraform").exists() or \
                     (cluster_dir / "main.tf").exists()

            if has_tf:
                clusters.append({
                    "provider": provider,
                    "cluster_name": cluster_name,
                    "work_dir": cluster_dir,
                })

    return clusters


def cleanup_cluster_state(provider: str, cluster_name: str, state_dir: Optional[str] = None):
    """
    Remove local state files for a cluster after destruction.

    Warning: This deletes the working directory and all state.
    Only call this after terraform destroy has completed successfully.
    """
    work_dir = get_state_dir(provider, cluster_name, state_dir)

    if work_dir.exists():
        console.print(f"[yellow]Cleaning up state directory: {work_dir}[/]")
        shutil.rmtree(work_dir)
        console.print("[green]State directory removed[/]")

        # Clean up empty parent directories
        provider_dir = work_dir.parent
        if provider_dir.exists() and not any(provider_dir.iterdir()):
            provider_dir.rmdir()
