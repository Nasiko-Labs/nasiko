#!/usr/bin/env python3
"""
Kong Kubernetes Service Registry - Automatic service discovery and registration for Kong API Gateway.

This service automatically discovers Kubernetes services in the agents namespace
and registers them as services and routes in Kong. It replaces the Docker-based
discovery with Kubernetes API-based discovery.
"""

import asyncio
import copy
import logging
import os
import time
from typing import List, Optional, Set

import requests
from fastapi import FastAPI, HTTPException
from kubernetes import client, config
from pydantic import BaseModel
from pythonjsonlogger import jsonlogger
import docker
from target_publisher import RedisTargetPublisher, build_target_record

# Configure logging
logger = logging.getLogger(__name__)
handler = logging.StreamHandler()
formatter = jsonlogger.JsonFormatter()
handler.setFormatter(formatter)
logger.addHandler(handler)
logger.setLevel(logging.INFO)

# Configuration
KONG_ADMIN_URL = os.getenv("KONG_ADMIN_URL", "http://kong-gateway:8001")
REGISTRY_INTERVAL = int(os.getenv("REGISTRY_INTERVAL", "30"))
AGENTS_NAMESPACE = os.getenv("AGENTS_NAMESPACE", "nasiko-agents")
PLATFORM_NAMESPACE = os.getenv("PLATFORM_NAMESPACE", "nasiko")
AGENTS_NETWORK = os.getenv("AGENTS_NETWORK", "agents-net")
REDIS_URL = os.getenv("REDIS_URL", "redis://redis:6379")
REQUEST_MANAGER_SERVICE_NAME = os.getenv(
    "KONG_REQUEST_MANAGER_SERVICE_NAME",
    "agent-request-manager",
)
REQUEST_MANAGER_HOST = os.getenv("KONG_REQUEST_MANAGER_HOST", "nasiko-request-manager")
REQUEST_MANAGER_PORT = int(os.getenv("KONG_REQUEST_MANAGER_PORT", "8090"))
REQUEST_MANAGER_ROUTE_PREFIX = os.getenv("KONG_REQUEST_MANAGER_ROUTE_PREFIX", "/agents")
K8S_ENABLED = os.getenv("K8S_ENABLED", "true").strip().lower() in {
    "1",
    "true",
    "yes",
    "y",
    "on",
}

if not K8S_ENABLED:
    logger.info("K8S_ENABLED=false; Using Docker container discovery")
else:
    logger.info("K8S_ENABLED=true; Using Kubernetes service discovery")

# FastAPI app for health checks and status
app = FastAPI(
    title="Kong Service Registry",
    version="1.0.0",
    description="Automatic service discovery and registration for Kong API Gateway. Supports both Kubernetes and Docker container discovery.",
)

# Global state
current_services: Set[str] = set()
k8s_client = None
docker_client = None
target_publisher = None


class ServiceInfo(BaseModel):
    name: str
    host: str
    port: int
    path: str = "/"
    methods: List[str] = ["GET", "POST", "PUT", "DELETE", "PATCH"]
    namespace: str
    source: str
    target_revision: str


class RegistryStatus(BaseModel):
    status: str
    services_count: int
    last_sync: Optional[str]
    kong_status: str


def get_k8s_client():
    """Initialize Kubernetes client."""
    global k8s_client
    if not K8S_ENABLED:
        return None
    if k8s_client is None:
        try:
            # Try in-cluster config first
            try:
                config.load_incluster_config()
                logger.info("Loaded in-cluster Kubernetes config")
            except config.ConfigException:
                # Fallback to kubeconfig
                config.load_kube_config()
                logger.info("Loaded kubeconfig")

            k8s_client = client.CoreV1Api()
            logger.info("Kubernetes client initialized successfully")
        except Exception as e:
            logger.error(f"Failed to initialize Kubernetes client: {e}")
            raise
    return k8s_client


def get_docker_client():
    """Initialize Docker client."""
    global docker_client
    if K8S_ENABLED:  # Only use Docker client when K8S is disabled
        return None
    if docker_client is None:
        try:
            docker_client = docker.from_env()
            # Test connection
            docker_client.ping()
            logger.info("Docker client initialized successfully")
        except Exception as e:
            logger.error(f"Failed to initialize Docker client: {e}")
            raise
    return docker_client


def get_target_publisher() -> RedisTargetPublisher | None:
    """Initialize Redis publisher for Request Manager target records."""
    global target_publisher
    if target_publisher is None:
        try:
            target_publisher = RedisTargetPublisher(REDIS_URL)
            logger.info("Request Manager target publisher initialized")
        except Exception as e:
            logger.error(f"Failed to initialize target publisher: {e}")
            return None
    return target_publisher


def check_kong_health() -> bool:
    """Check if Kong is healthy and accessible."""
    try:
        response = requests.get(f"{KONG_ADMIN_URL}/status", timeout=5)
        return response.status_code == 200
    except Exception as e:
        logger.warning(f"Kong health check failed: {e}")
        return False


def get_service_port(svc):
    """Smart port discovery using named ports first, fallback to first port."""
    if not svc.spec.ports:
        return None

    # Strategy 1: Look for named ports that indicate HTTP APIs
    for port in svc.spec.ports:
        if port.name and port.name.lower() in ["http", "api", "web", "rest"]:
            return port.port

    # Fallback: Use first port
    return svc.spec.ports[0].port


def get_k8s_services() -> List[ServiceInfo]:
    """Discover agent services from Kubernetes in the agents namespace only."""
    services = []

    try:
        if not K8S_ENABLED:
            return services
        k8s = get_k8s_client()
        if k8s is None:
            return services

        # Get services ONLY in agents namespace
        try:
            agents_services = k8s.list_namespaced_service(namespace=AGENTS_NAMESPACE)
        except client.exceptions.ApiException as e:
            if e.status == 404:
                logger.warning(f"Namespace '{AGENTS_NAMESPACE}' not found")
                return services
            raise

        for svc in agents_services.items:
            try:
                service_name = svc.metadata.name

                # Skip kubernetes system services
                if service_name in [
                    "kubernetes",
                    "kube-dns",
                    "kube-proxy",
                    "metrics-server",
                    "coredns",
                ]:
                    continue

                # Skip services that are likely StatefulSets or databases
                if any(
                    pattern in service_name.lower()
                    for pattern in [
                        "headless",
                        "postgres",
                        "redis",
                        "mongodb",
                        "mysql",
                        "elasticsearch",
                        "kafka",
                        "zookeeper",
                        "cassandra",
                        "etcd",
                    ]
                ):
                    continue

                # Skip headless services (StatefulSets often use these)
                if svc.spec.cluster_ip == "None":
                    continue

                # Use smart port discovery
                service_port = get_service_port(svc)
                if service_port is None:
                    logger.warning(f"Service {service_name} has no ports defined")
                    continue

                # Use service DNS name for internal cluster communication
                service_host = f"{service_name}.{AGENTS_NAMESPACE}.svc.cluster.local"

                service_info = ServiceInfo(
                    name=service_name,
                    host=service_host,
                    port=service_port,
                    path=f"/agents/{service_name}",
                    methods=["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
                    namespace=AGENTS_NAMESPACE,
                    source="kubernetes",
                    target_revision=svc.metadata.resource_version
                    or svc.metadata.uid
                    or service_name,
                )

                services.append(service_info)
                logger.info(
                    f"Discovered agent service: {service_name} at {service_host}:{service_port}"
                )

            except Exception as e:
                logger.error(f"Error processing service {svc.metadata.name}: {e}")
                continue

    except Exception as e:
        logger.error(f"Error discovering services: {e}")

    return services


def get_docker_services() -> List[ServiceInfo]:
    """Discover agent containers from Docker on the agents network."""
    services = []

    try:
        if K8S_ENABLED:  # Only discover Docker containers when K8S is disabled
            return services

        docker_client = get_docker_client()
        if docker_client is None:
            return services

        # Get all containers connected to the agents network
        try:
            # Get the agents network
            agents_network = docker_client.networks.get(AGENTS_NETWORK)
        except docker.errors.NotFound:
            logger.warning(f"Docker network '{AGENTS_NETWORK}' not found")
            return services

        # Get containers connected to this network
        containers = agents_network.containers

        for container in containers:
            try:
                # Refresh container info
                container.reload()

                container_name = container.name
                container_status = container.status

                # Skip non-running containers
                if container_status != "running":
                    logger.debug(
                        f"Skipping non-running container: {container_name} (status: {container_status})"
                    )
                    continue

                # Skip Kong itself and other infrastructure containers
                if container_name in [
                    "kong-gateway",
                    "kong-database",
                    "kong-migrations",
                    "kong-service-registry",
                    "nasiko-backend",
                    "nasiko-web",
                    "nasiko-router",
                    "nasiko-auth-service",
                    "nasiko-chat-history",
                    "nasiko-request-manager",
                    "redis",
                    "mongodb",
                    "phoenix-observability",
                ]:
                    logger.debug(f"Skipping infrastructure container: {container_name}")
                    continue

                # Only consider agent containers (those that start with 'agent-')
                if not container_name.startswith("agent-"):
                    logger.debug(f"Skipping non-agent container: {container_name}")
                    continue

                # For Docker containers on the same network, we don't need exposed ports
                # We can communicate directly using container name and internal port
                # Assume agents run on port 5000 internally (like in K8s)
                service_port = 5000

                # Optional: Verify the container is actually listening on port 5000
                # by checking container labels or image metadata, but for now assume it is

                # Use container name as hostname for network communication
                service_host = container_name

                service_info = ServiceInfo(
                    name=container_name,
                    host=service_host,
                    port=service_port,
                    path=f"/agents/{container_name}",
                    methods=["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
                    namespace="docker-agents",  # Use a different namespace for Docker containers
                    source="docker",
                    target_revision=container.id,
                )

                services.append(service_info)
                logger.info(
                    f"Discovered agent container: {container_name} at {service_host}:{service_port}"
                )

            except Exception as e:
                logger.error(f"Error processing container {container.name}: {e}")
                continue

    except Exception as e:
        logger.error(f"Error discovering Docker containers: {e}")

    return services


def ensure_request_manager_service() -> bool:
    """Ensure the shared Kong service for all dynamic agent routes exists."""
    service_data = {
        "name": REQUEST_MANAGER_SERVICE_NAME,
        "url": f"http://{REQUEST_MANAGER_HOST}:{REQUEST_MANAGER_PORT}",
        "connect_timeout": 60000,
        "write_timeout": 300000,
        "read_timeout": 300000,
        "retries": 0,
        "protocol": "http",
    }

    try:
        response = requests.get(
            f"{KONG_ADMIN_URL}/services/{REQUEST_MANAGER_SERVICE_NAME}", timeout=10
        )
        if response.status_code == 200:
            response = requests.patch(
                f"{KONG_ADMIN_URL}/services/{REQUEST_MANAGER_SERVICE_NAME}",
                json=service_data,
                timeout=10,
            )
            logger.info("Updated Request Manager Kong service")
        else:
            response = requests.post(
                f"{KONG_ADMIN_URL}/services", json=service_data, timeout=10
            )
            logger.info("Created Request Manager Kong service")
        return response.status_code in [200, 201]
    except Exception as e:
        logger.error(f"Failed to ensure Request Manager Kong service: {e}")
        return False


def register_service_in_kong(service: ServiceInfo) -> bool:
    """Register an agent route in Kong that points to the Request Manager."""
    try:
        if not ensure_request_manager_service():
            return False

        route_data = {
            "name": f"{service.name}-route",
            "paths": [service.path],
            "methods": service.methods,
            "strip_path": False,
            "preserve_host": False,
            "service": {"name": REQUEST_MANAGER_SERVICE_NAME},
        }

        response = requests.get(
            f"{KONG_ADMIN_URL}/routes/{service.name}-route", timeout=10
        )
        if response.status_code == 200:
            response = requests.patch(
                f"{KONG_ADMIN_URL}/routes/{service.name}-route",
                json=route_data,
                timeout=10,
            )
            logger.info(f"Updated dynamic Request Manager route: {service.name}-route")
        else:
            response = requests.post(
                f"{KONG_ADMIN_URL}/services/{REQUEST_MANAGER_SERVICE_NAME}/routes",
                json=route_data,
                timeout=10,
            )
            logger.info(f"Created dynamic Request Manager route: {service.name}-route")

        if response.status_code not in [200, 201]:
            logger.error(
                f"Failed to register Request Manager route for {service.name}: {response.text}"
            )
            return False

        logger.info(f"Registered {service.path} through Request Manager")
        return True
    except Exception as e:
        logger.error(f"Error registering Request Manager route for {service.name}: {e}")
        return False


def cleanup_stale_services(current_service_names: Set[str]) -> None:
    """Remove services from Kong that no longer have running containers."""
    try:
        # Get all services from Kong
        response = requests.get(f"{KONG_ADMIN_URL}/services", timeout=10)
        if response.status_code != 200:
            logger.error(f"Failed to get services from Kong: {response.text}")
            return

        kong_services = response.json().get("data", [])

        # Static proxy services that should never be cleaned up
        static_proxy_services = {
            "backend-api-proxy",
            "web-app-proxy",
            "auth-proxy",
            "nasiko-router",
            "landing-page",
            "n8n",
            "gateway-status",
            "gateway-health",
            REQUEST_MANAGER_SERVICE_NAME,
        }

        for kong_service in kong_services:
            service_name = kong_service["name"]

            # Skip Kong's internal services
            if service_name in ["kong", "postgres", "konga", "registry"]:
                continue

            # Skip static proxy services - they're not k8s services but should be preserved
            if service_name in static_proxy_services:
                continue

            if (
                service_name.startswith("agent-")
                and service_name != REQUEST_MANAGER_SERVICE_NAME
            ):
                try:
                    routes_response = requests.get(
                        f"{KONG_ADMIN_URL}/services/{service_name}/routes",
                        timeout=10,
                    )
                    if routes_response.status_code == 200:
                        for route in routes_response.json().get("data", []):
                            requests.delete(
                                f"{KONG_ADMIN_URL}/routes/{route['id']}", timeout=10
                            )
                    delete_response = requests.delete(
                        f"{KONG_ADMIN_URL}/services/{service_name}",
                        timeout=10,
                    )
                    if delete_response.status_code == 204:
                        logger.info(
                            f"Removed legacy direct agent service: {service_name}"
                        )
                except Exception as e:
                    logger.error(
                        f"Error removing legacy direct agent service {service_name}: {e}"
                    )
                continue

            # If service is not in current running containers, remove it
            if service_name not in current_service_names:
                try:
                    # Delete routes first
                    routes_response = requests.get(
                        f"{KONG_ADMIN_URL}/services/{service_name}/routes"
                    )
                    if routes_response.status_code == 200:
                        routes = routes_response.json().get("data", [])
                        for route in routes:
                            delete_response = requests.delete(
                                f"{KONG_ADMIN_URL}/routes/{route['id']}"
                            )
                            if delete_response.status_code == 204:
                                logger.info(
                                    f"Deleted route {route['name']} for service {service_name}"
                                )

                    # Delete service
                    delete_response = requests.delete(
                        f"{KONG_ADMIN_URL}/services/{service_name}"
                    )
                    if delete_response.status_code == 204:
                        logger.info(f"Removed stale service: {service_name}")

                except Exception as e:
                    logger.error(f"Error removing stale service {service_name}: {e}")

    except Exception as e:
        logger.error(f"Error cleaning up stale services: {e}")


def cleanup_stale_agent_routes(current_service_names: Set[str]) -> None:
    """Remove dynamic agent routes that no longer have discovered targets."""
    try:
        response = requests.get(f"{KONG_ADMIN_URL}/routes", timeout=10)
        if response.status_code != 200:
            logger.error(f"Failed to get Kong routes: {response.text}")
            return

        valid_route_names = {f"{name}-route" for name in current_service_names}
        for route in response.json().get("data", []):
            route_name = route.get("name", "")
            if not route_name.endswith("-route"):
                continue
            if not route_name.startswith("agent-"):
                continue
            if route_name in valid_route_names:
                continue
            delete_response = requests.delete(
                f"{KONG_ADMIN_URL}/routes/{route['id']}", timeout=10
            )
            if delete_response.status_code == 204:
                logger.info(f"Deleted stale agent route: {route_name}")
    except Exception as e:
        logger.error(f"Error cleaning up stale agent routes: {e}")


def configure_kong_plugins():
    """Configure Kong plugins on startup - no global plugins, only per-route."""
    logger.info("Kong plugin configuration: All plugins will be applied per-route only")


def _resolve_service_host(k8s_service: str, local_service: str, env_var: str) -> str:
    """Resolve service host for Kong based on K8S_ENABLED or explicit override."""
    override = os.getenv(env_var)
    if override:
        return override
    if K8S_ENABLED:
        return f"{k8s_service}.{PLATFORM_NAMESPACE}.svc.cluster.local"
    return local_service


def register_static_proxies():
    """Register static proxy services for auth, chat, and default router."""
    logger.info("Configuring static proxy routes...")

    backend_host = _resolve_service_host(
        k8s_service="nasiko-backend",
        local_service="nasiko-backend",
        env_var="KONG_BACKEND_HOST",
    )
    web_host = _resolve_service_host(
        k8s_service="nasiko-web",
        local_service="nasiko-web",
        env_var="KONG_WEB_HOST",
    )
    web_path = os.getenv("KONG_WEB_PATH", "/app").strip() or "/app"
    if not web_path.startswith("/"):
        web_path = f"/{web_path}"
    auth_host = _resolve_service_host(
        k8s_service="nasiko-auth",
        local_service="nasiko-auth-service-oss",
        env_var="KONG_AUTH_HOST",
    )
    router_host = _resolve_service_host(
        k8s_service="nasiko-router",
        local_service="nasiko-router",
        env_var="KONG_ROUTER_HOST",
    )
    n8n_host = _resolve_service_host(
        k8s_service="n8n",
        local_service="n8n",
        env_var="KONG_N8N_HOST",
    )

    static_services = [
        # Backend API proxy - auth only, no chat logging
        {
            "name": "backend-api-proxy",
            "host": backend_host,
            "port": 8000,
            "paths": ["/api"],
            "methods": ["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
            "strip_path": False,  # /api/v1/users → /api/v1/users
            "preserve_host": False,
            "middlewares": ["cors", "nasiko-auth"],  # CORS + Auth, no chat logging
        },
        # Web app proxy - no middleware
        {
            "name": "web-app-proxy",
            "host": web_host,
            "port": 4000,
            "paths": [web_path],
            "methods": ["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
            "strip_path": True,
            "preserve_host": False,
            "middlewares": ["cors"],  # CORS only
        },
        # Auth service proxy - auth only
        {
            "name": "auth-proxy",
            "host": auth_host,
            "port": 8001,
            "paths": ["/auth"],
            "methods": ["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
            "strip_path": False,  # /auth/login → /login
            "preserve_host": False,
            # Auth endpoints (login, token exchange, GitHub OAuth callbacks) must be reachable
            # without already having an Authorization header.
            "middlewares": ["cors"],
        },
        # Chat service proxy (sidecar) - no middleware. -- we don't need
        # {
        #     "name": "chat-proxy",
        #     "host": "localhost",  # Sidecar in same pod
        #     "port": 8002,
        #     "paths": ["/chat"],
        #     "methods": ["GET", "POST"],
        #     "strip_path": True,  # /chat/history → /chat-history
        #     "preserve_host": False,
        #     "middlewares": []  # No middleware
        # },
        # Router service - mapped to /router path
        {
            "name": "nasiko-router",
            "host": router_host,
            "port": 8000,
            "paths": ["/router"],
            "upstream_path": "",  # No upstream path to avoid double /router
            "methods": ["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
            "strip_path": False,  # Keep /router in path when forwarding
            "preserve_host": False,
            "middlewares": [
                "cors",
                "nasiko-auth",
                "chat-logger",
            ],  # Full middleware with CORS
        },
        # Static landing page - forward to web app
        {
            "name": "landing-page",
            "host": web_host,
            "port": 4000,
            "paths": ["/"],
            "upstream_path": "/app/",
            "methods": ["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
            "strip_path": False,
            "preserve_host": True,
            "middlewares": ["cors"],  # CORS only
        },
        # Gateway status passthrough for local proxy health checks
        {
            "name": "gateway-status",
            "host": "kong-service-registry",
            "port": 8080,
            "paths": ["/status"],
            "methods": ["GET", "OPTIONS"],
            "strip_path": False,
            "preserve_host": False,
            "middlewares": ["cors"],
        },
        # Gateway health passthrough for local proxy health checks
        {
            "name": "gateway-health",
            "host": "kong-service-registry",
            "port": 8080,
            "paths": ["/health"],
            "methods": ["GET", "OPTIONS"],
            "strip_path": False,
            "preserve_host": False,
            "middlewares": ["cors"],
        },
        # N8N workflow automation service
        {
            "name": "n8n",
            "host": n8n_host,
            "port": 5678,
            "paths": ["/n8n"],
            "methods": ["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
            "strip_path": True,
            "preserve_host": False,
            "middlewares": [
                "cors"
            ],  # CORS only, no auth for now (n8n has its own auth)
        },
    ]

    for service_config in static_services:
        try:
            register_proxy_service_in_kong(service_config)
        except Exception as e:
            logger.error(
                f"Error registering static proxy {service_config['name']}: {e}"
            )


def register_proxy_service_in_kong(service_config):
    """Register a proxy service in Kong with array-based middleware system."""
    try:
        logger.info(f"Starting registration of proxy service: {service_config['name']}")

        # Create service URL
        if service_config.get("upstream_path"):
            service_url = f"http://{service_config['host']}:{service_config['port']}{service_config['upstream_path']}"
        else:
            service_url = f"http://{service_config['host']}:{service_config['port']}"

        logger.info(f"Service URL for {service_config['name']}: {service_url}")

        # Create service in Kong
        service_data = {
            "name": service_config["name"],
            "url": service_url,
            "connect_timeout": 60000,
            "write_timeout": 300000,
            "read_timeout": 300000,
            "retries": 3,
            "protocol": "http",
        }

        logger.info(f"Service data for {service_config['name']}: {service_data}")

        # Check if service exists
        try:
            logger.info(f"Checking if service {service_config['name']} exists...")
            response = requests.get(
                f"{KONG_ADMIN_URL}/services/{service_config['name']}"
            )
            logger.info(
                f"Service check response for {service_config['name']}: {response.status_code}"
            )

            if response.status_code == 200:
                # Update existing service
                logger.info(f"Updating existing service {service_config['name']}...")
                response = requests.patch(
                    f"{KONG_ADMIN_URL}/services/{service_config['name']}",
                    json=service_data,
                    timeout=10,
                )
                logger.info(
                    f"Update response for {service_config['name']}: {response.status_code} - {response.text[:200]}"
                )
                logger.info(f"Updated existing proxy service: {service_config['name']}")
            else:
                # Create new service
                logger.info(f"Creating new service {service_config['name']}...")
                response = requests.post(
                    f"{KONG_ADMIN_URL}/services", json=service_data, timeout=10
                )
                logger.info(
                    f"Create response for {service_config['name']}: {response.status_code} - {response.text[:200]}"
                )
                logger.info(f"Created new proxy service: {service_config['name']}")
        except requests.exceptions.RequestException as e:
            # Create new service
            logger.warning(
                f"Request exception checking service {service_config['name']}: {e}"
            )
            logger.info(
                f"Creating new service {service_config['name']} due to exception..."
            )
            response = requests.post(
                f"{KONG_ADMIN_URL}/services", json=service_data, timeout=10
            )
            logger.info(
                f"Exception create response for {service_config['name']}: {response.status_code} - {response.text[:200]}"
            )
            logger.info(f"Created new proxy service: {service_config['name']}")

        if response.status_code not in [200, 201]:
            logger.error(
                f"Failed to register proxy service {service_config['name']}: HTTP {response.status_code} - {response.text}"
            )
            return False

        # Create route for the service
        route_data = {
            "name": f"{service_config['name']}-route",
            "paths": service_config["paths"],
            "methods": service_config["methods"],
            "strip_path": service_config.get("strip_path", False),
            "preserve_host": service_config.get("preserve_host", False),
        }

        logger.info(f"Route data for {service_config['name']}: {route_data}")

        # Check if route exists
        try:
            logger.info(f"Checking if route {service_config['name']}-route exists...")
            response = requests.get(
                f"{KONG_ADMIN_URL}/routes/{service_config['name']}-route"
            )
            logger.info(
                f"Route check response for {service_config['name']}: {response.status_code}"
            )

            if response.status_code == 200:
                # Update existing route
                logger.info(
                    f"Updating existing route {service_config['name']}-route..."
                )
                response = requests.patch(
                    f"{KONG_ADMIN_URL}/routes/{service_config['name']}-route",
                    json=route_data,
                    timeout=10,
                )
                logger.info(
                    f"Route update response for {service_config['name']}: {response.status_code} - {response.text[:200]}"
                )
                logger.info(
                    f"Updated existing proxy route: {service_config['name']}-route"
                )
            else:
                # Create new route
                logger.info(f"Creating new route {service_config['name']}-route...")
                route_data["service"] = {"name": service_config["name"]}
                response = requests.post(
                    f"{KONG_ADMIN_URL}/services/{service_config['name']}/routes",
                    json=route_data,
                    timeout=10,
                )
                logger.info(
                    f"Route create response for {service_config['name']}: {response.status_code} - {response.text[:200]}"
                )
                logger.info(f"Created new proxy route: {service_config['name']}-route")
        except requests.exceptions.RequestException as e:
            # Create new route
            logger.warning(
                f"Request exception checking route {service_config['name']}: {e}"
            )
            logger.info(
                f"Creating new route {service_config['name']}-route due to exception..."
            )
            route_data["service"] = {"name": service_config["name"]}
            response = requests.post(
                f"{KONG_ADMIN_URL}/services/{service_config['name']}/routes",
                json=route_data,
                timeout=10,
            )
            logger.info(
                f"Route exception create response for {service_config['name']}: {response.status_code} - {response.text[:200]}"
            )
            logger.info(f"Created new proxy route: {service_config['name']}-route")

        if response.status_code not in [200, 201]:
            logger.error(
                f"Failed to register proxy route for {service_config['name']}: HTTP {response.status_code} - {response.text}"
            )
            return False

        # Apply middlewares in array order
        middlewares = service_config.get("middlewares", [])
        if middlewares:
            route_name = f"{service_config['name']}-route"
            apply_middlewares_to_route(route_name, middlewares)
        else:
            logger.info(
                f"No middlewares configured for route: {service_config['name']}-route"
            )

        # Verify the service and route were actually created
        logger.info(f"Verifying registration of {service_config['name']}...")
        try:
            service_check = requests.get(
                f"{KONG_ADMIN_URL}/services/{service_config['name']}"
            )
            route_check = requests.get(
                f"{KONG_ADMIN_URL}/routes/{service_config['name']}-route"
            )

            logger.info(
                f"Post-registration verification - Service {service_config['name']}: {service_check.status_code}"
            )
            logger.info(
                f"Post-registration verification - Route {service_config['name']}-route: {route_check.status_code}"
            )

            if service_check.status_code == 200 and route_check.status_code == 200:
                logger.info(
                    f"Successfully registered and verified proxy {service_config['name']} in Kong"
                )
            else:
                logger.error(
                    f"Registration verification failed for {service_config['name']} - Service: {service_check.status_code}, Route: {route_check.status_code}"
                )
                return False
        except Exception as e:
            logger.error(f"Error during verification of {service_config['name']}: {e}")
            return False

        return True

    except Exception as e:
        logger.error(
            f"Error registering proxy service {service_config['name']} in Kong: {e}"
        )
        return False


def apply_middlewares_to_route(route_name, middlewares):
    """Apply middlewares to a specific route in order."""
    logger.info(f"Applying middlewares to route {route_name}: {middlewares}")

    # Used by the nasiko-auth Kong plugin to call the auth service.
    # Keep this environment-overridable so local dev (docker-compose) doesn't depend on doctl contexts
    # or Kubernetes-only DNS names.
    auth_service_url = os.getenv("KONG_AUTH_SERVICE_URL")
    if not auth_service_url:
        auth_host = _resolve_service_host(
            k8s_service="nasiko-auth",
            local_service="nasiko-auth-service-oss",
            env_var="KONG_AUTH_HOST",
        )
        auth_port = os.getenv("KONG_AUTH_PORT", "8001").strip() or "8001"
        auth_service_url = f"http://{auth_host}:{auth_port}"

    # Plugin name mapping
    plugin_configs = {
        "nasiko-auth": {
            "name": "nasiko-auth",
            "config": {"auth_service_url": auth_service_url, "timeout": 5000},
        },
        "chat-logger": {
            "name": "chat-logger",
            "config": {
                "chat_service_url": (
                    "http://localhost:8002"
                    if K8S_ENABLED
                    else "http://nasiko-chat-history:8002"
                ),
                "timeout": 5000,
            },
        },
        "cors": {
            "name": "cors",
            "config": {
                "origins": ["*"],
                "methods": ["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
                "headers": [
                    "Accept",
                    "Accept-Version",
                    "Content-Length",
                    "Content-MD5",
                    "Content-Type",
                    "Date",
                    "Authorization",
                    "X-Auth-Token",
                    "X-Requested-With",
                ],
                "exposed_headers": [
                    "X-Subject-ID",
                    "X-Subject-Type",
                    "X-Is-Super-User",
                    "X-Permissions",
                    "X-Request-Layer-Agent",
                    "X-Request-Layer-Cache",
                    "X-Request-Layer-Queue-Wait-Ms",
                    "X-Request-Layer-Limit-State",
                ],
                "credentials": True,
                "max_age": 3600,
                "preflight_continue": False,
            },
        },
        "static-landing": {
            "name": "request-termination",
            "config": {
                "status_code": 200,
                "content_type": "text/html",
                "body": """<!DOCTYPE html>
<html>
<head>
    <meta http-equiv="refresh" content="0; url=/app" />
    <title>Redirecting...</title>
</head>
<body>
    <p>Redirecting to <a href="/app">/app</a>...</p>
    <script>window.location.href = "/app"</script>
</body>
</html>""",
            },
        },
    }

    for middleware in middlewares:
        try:
            if middleware not in plugin_configs:
                logger.warning(f"Unknown middleware: {middleware}, skipping...")
                continue

            plugin_config = copy.deepcopy(plugin_configs[middleware])
            plugin_config["route"] = {"name": route_name}

            # Always create new plugin (avoid update issues)
            response = requests.post(
                f"{KONG_ADMIN_URL}/plugins", json=plugin_config, timeout=10
            )

            if response.status_code in [200, 201]:
                logger.info(f"Applied {middleware} plugin to route {route_name}")
            elif response.status_code == 409:
                # Plugin already exists - this is expected, not an error
                logger.info(
                    f"{middleware} plugin already exists for route {route_name}"
                )
            else:
                logger.error(
                    f"Failed to apply {middleware} to route {route_name}: {response.text}"
                )

        except Exception as e:
            logger.error(
                f"Error applying middleware {middleware} to route {route_name}: {e}"
            )


async def sync_services():
    """Main sync loop - discover services and register them with Kong."""
    global current_services

    logger.info("Starting service synchronization")

    # Configure plugins on first run
    plugin_configured = False

    while True:
        try:
            # Check Kong health
            if not check_kong_health():
                logger.warning("Kong is not healthy, skipping sync")
                await asyncio.sleep(REGISTRY_INTERVAL)
                continue

            # Configure plugins and static proxies on first successful Kong connection
            if not plugin_configured:
                try:
                    configure_kong_plugins()
                    register_static_proxies()
                    plugin_configured = True
                except Exception as e:
                    logger.error(f"Plugin/proxy configuration failed: {e}")
                # Continue even if plugin configuration fails

            # Discover services based on deployment type
            if K8S_ENABLED:
                services = get_k8s_services()
                logger.debug(f"Discovered {len(services)} Kubernetes services")
            else:
                services = get_docker_services()
                logger.debug(f"Discovered {len(services)} Docker containers")

            publisher = get_target_publisher()
            if publisher:
                target_records = [
                    build_target_record(
                        agent_id=service.name,
                        host=service.host,
                        port=service.port,
                        public_path=service.path,
                        namespace=service.namespace,
                        source=service.source,
                        target_revision=service.target_revision,
                    )
                    for service in services
                ]
                try:
                    publisher.publish(target_records)
                except Exception as e:
                    logger.error(f"Failed to publish Request Manager targets: {e}")

            # Register/update services (dynamic agents with full middleware)
            successful_registrations = set()
            for service in services:
                if register_service_in_kong(service):
                    successful_registrations.add(service.name)

                    # Apply full middleware to dynamic agent routes
                    route_name = f"{service.name}-route"
                    apply_middlewares_to_route(
                        route_name, ["cors", "nasiko-auth", "chat-logger"]
                    )

            # Clean up stale services
            cleanup_stale_services(successful_registrations)
            cleanup_stale_agent_routes(successful_registrations)

            # Update current services
            current_services = successful_registrations

            logger.info(f"Sync completed. Active services: {len(current_services)}")

        except Exception as e:
            logger.error(f"Error in sync loop: {e}")

        await asyncio.sleep(REGISTRY_INTERVAL)


@app.on_event("startup")
async def startup_event():
    """Start the service sync loop."""
    logger.info("Kong Service Registry starting up")
    asyncio.create_task(sync_services())


@app.get("/health")
async def health_check():
    """Health check endpoint."""
    return {"status": "healthy", "timestamp": time.time()}


@app.get("/status")
async def get_status() -> RegistryStatus:
    """Get registry status."""
    kong_healthy = check_kong_health()

    return RegistryStatus(
        status="healthy" if kong_healthy else "degraded",
        services_count=len(current_services),
        last_sync=time.strftime("%Y-%m-%d %H:%M:%S"),
        kong_status="healthy" if kong_healthy else "unhealthy",
    )


@app.get("/services")
async def list_services():
    """List currently registered services."""
    return {"services": list(current_services)}


@app.post("/sync")
async def trigger_sync():
    """Manually trigger a service sync."""
    try:
        # Discover services based on deployment type
        if K8S_ENABLED:
            services = get_k8s_services()
            discovery_type = "Kubernetes"
        else:
            services = get_docker_services()
            discovery_type = "Docker"

        publisher = get_target_publisher()
        if publisher:
            target_records = [
                build_target_record(
                    agent_id=service.name,
                    host=service.host,
                    port=service.port,
                    public_path=service.path,
                    namespace=service.namespace,
                    source=service.source,
                    target_revision=service.target_revision,
                )
                for service in services
            ]
            try:
                publisher.publish(target_records)
            except Exception as e:
                logger.error(f"Failed to publish Request Manager targets: {e}")

        registered = 0
        for service in services:
            if register_service_in_kong(service):
                registered += 1
                route_name = f"{service.name}-route"
                apply_middlewares_to_route(
                    route_name, ["cors", "nasiko-auth", "chat-logger"]
                )

        return {
            "message": f"Sync completed. Registered {registered} {discovery_type} services."
        }

    except Exception as e:
        logger.error(f"Manual sync failed: {e}")
        raise HTTPException(status_code=500, detail=str(e))


if __name__ == "__main__":
    import uvicorn

    logger.info("Starting Kong Service Registry")
    uvicorn.run(app, host="0.0.0.0", port=8080, log_level="info")
