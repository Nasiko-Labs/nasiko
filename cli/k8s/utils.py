"""
Generic utilities for Kubernetes and Helm operations.
"""

import os
import tempfile
import yaml
from pathlib import Path
from rich.console import Console

console = Console()


def create_dynamic_helm_values(values_dict):
    """Create a temporary YAML values file from dictionary

    Args:
        values_dict: Dictionary containing Helm values

    Returns:
        str: Path to the temporary values file

    Raises:
        Exception: If file creation fails
    """
    values_file = None
    try:
        with tempfile.NamedTemporaryFile(mode='w', suffix='.yaml', delete=False) as f:
            yaml.dump(values_dict, f, default_flow_style=False)
            values_file = f.name
        console.print(f"[cyan]Generated dynamic values file: {values_file}[/]")
        return values_file
    except Exception as e:
        console.print(f"[red]Failed to create values file: {e}[/]")
        if values_file and os.path.exists(values_file):
            os.unlink(values_file)
        raise


def deploy_helm_chart(
    chart_name,
    chart_path,
    release_name,
    dynamic_values,
    namespace="nasiko",
    environment="default",
    additional_args=None,
    helm_runner=None
):
    """Generic function to deploy any Helm chart with dynamic values

    Args:
        chart_name: Human-readable name for logging
        chart_path: Path to the Helm chart directory
        release_name: Helm release name
        dynamic_values: Dictionary of values to inject
        namespace: Kubernetes namespace (default: nasiko)
        environment: Environment name for values-{env}.yaml file
        additional_args: List of additional Helm arguments
        helm_runner: Function to run helm commands (required, no default import to avoid circular deps)
    """
    if helm_runner is None:
        raise ValueError("helm_runner function must be provided to deploy_helm_chart")

    # Note: ensure_helm() should be called by the caller before calling this function

    values_file = None
    try:
        # Create dynamic values file
        values_file = create_dynamic_helm_values(dynamic_values)

        # Build helm command
        helm_cmd = [
            "upgrade", "--install", release_name,
            str(chart_path),
            "--namespace", namespace,
            "--create-namespace",
            "-f", values_file
        ]

        # Add environment-specific values file if it exists
        env_values_file = Path(chart_path) / f"values-{environment}.yaml"
        if env_values_file.exists():
            helm_cmd.extend(["-f", str(env_values_file)])
            console.print(f"[cyan]Using environment values: {env_values_file}[/]")

        # Add any additional helm arguments
        if additional_args:
            helm_cmd.extend(additional_args)

        helm_runner(helm_cmd, f"{chart_name} via Helm")

    finally:
        # Clean up temporary values file
        if values_file and os.path.exists(values_file):
            os.unlink(values_file)


def cleanup_helm_values_file(values_file):
    """Clean up a temporary values file

    Args:
        values_file: Path to the values file to clean up
    """
    if values_file and os.path.exists(values_file):
        try:
            os.unlink(values_file)
            console.print(f"[dim]Cleaned up values file: {values_file}[/]")
        except Exception as e:
            console.print(f"[yellow]Warning: Could not clean up values file {values_file}: {e}[/]")


def validate_helm_values(values_dict, required_keys=None):
    """Validate that required keys exist in the values dictionary

    Args:
        values_dict: Dictionary to validate
        required_keys: List of required key paths (e.g., ["image.repository", "config.mongoUrl"])

    Returns:
        bool: True if validation passes

    Raises:
        ValueError: If validation fails
    """
    if not required_keys:
        return True

    missing_keys = []

    for key_path in required_keys:
        keys = key_path.split('.')
        current = values_dict

        try:
            for key in keys:
                current = current[key]
        except (KeyError, TypeError):
            missing_keys.append(key_path)

    if missing_keys:
        raise ValueError(f"Missing required values: {', '.join(missing_keys)}")

    return True