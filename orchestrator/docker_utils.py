"""
Docker Utility Functions
Common Docker command wrappers and utilities.
"""

import subprocess
import time
import logging

logger = logging.getLogger(__name__)

def run_cmd(cmd, check=True):
    """Execute a subprocess command"""
    logger.info(f"Running: {' '.join(cmd)}")
    return subprocess.run(cmd, check=check, capture_output=True, text=True)

def wait_for_health(container_name, timeout=60):
    """Wait for container to be running"""
    logger.info(f"Waiting for {container_name} to be running...")
    start = time.time()
    while time.time() - start < timeout:
        result = subprocess.run(
            ["docker", "inspect", "-f", "{{.State.Status}}", container_name],
            capture_output=True, text=True
        )
        if "running" in result.stdout:
            logger.info(f"{container_name} is running!")
            return True
        time.sleep(2)
    
    logger.error(f"{container_name} did not become running in {timeout}s")
    return False

def get_container_host_port(container_name, container_port="5000"):
    """Get the host port that Docker assigned to a container's port

    DEPRECATED: Use get_kong_agent_url() instead for agent registry URLs
    This function is kept for backward compatibility and direct container access
    """
    try:
        result = run_cmd([
            "docker", "port", container_name, container_port
        ])
        logger.info("Container name: %s, Port mapping: %s", container_name, result.stdout.strip())
        # Output format: "0.0.0.0:32768"
        port_mapping = result.stdout.strip()
        if port_mapping:
            # Extract just the port number
            host_port = port_mapping.split(':')[-1]
            return f"http://localhost:{host_port}"
        else:
            logger.warning(f"No port mapping found for {container_name}:{container_port}")
            return f"http://{container_name}"

    except subprocess.CalledProcessError as e:
        logger.error(f"Failed to get port mapping for {container_name}: {e}")
        return f"http://{container_name}"

def get_kong_agent_url(agent_id):
    """Get the Kong gateway URL for an agent

    Returns the Kong gateway URL that routes to the agent through the API gateway.
    This should be used for agent registry URLs as it provides external access.

    Args:
        agent_id (str): ID of the agent (used for Kong routing)

    Returns:
        str: Kong gateway URL for the agent
    """
    import socket

    # Get the private IP dynamically
    try:
        # Connect to a remote address to determine which interface to use
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.connect(("8.8.8.8", 80))
            private_ip = s.getsockname()[0]
    except Exception as e:
        logger.warning(f"Could not determine private IP: {e}, falling back to localhost")
        private_ip = "localhost"

    kong_url = f"http://{private_ip}:9100/{agent_id}"
    logger.info("Agent ID: %s, Kong gateway URL: %s", agent_id, kong_url)
    return kong_url

def network_exists(network_name):
    """Check if a Docker network exists"""
    try:
        result = run_cmd([
            "docker", "network", "ls", "--filter", f"name={network_name}", "--format", "{{.Name}}"
        ])
        return network_name in result.stdout
    except subprocess.CalledProcessError:
        return False

def create_network(network_name):
    """Create a Docker network if it doesn't exist"""
    if not network_exists(network_name):
        try:
            run_cmd(["docker", "network", "create", network_name])
            logger.info(f"Created network: {network_name}")
            return True
        except subprocess.CalledProcessError as e:
            logger.error(f"Failed to create network {network_name}: {e}")
            return False
    else:
        logger.info(f"Network {network_name} already exists")
        return True