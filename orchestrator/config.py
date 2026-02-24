"""
Orchestrator Configuration
Contains all constants and configuration settings.
"""

import os

class Config:
    # Docker Configuration
    DOCKER_NETWORK = os.getenv("DOCKER_NETWORK", "nasiko-network")
    NASIKO_API_URL = os.getenv("NASIKO_API_URL", "http://localhost:8000")
    KONG_GATEWAY_URL = os.getenv("KONG_GATEWAY_URL", "http://localhost:9100")

    # Agent Registry Configuration (for pre-built images)
    AGENT_REGISTRY_URL = os.getenv("AGENT_REGISTRY_URL", "docker.io")
    AGENT_IMAGE_TAG = os.getenv("AGENT_IMAGE_TAG", "latest")

    # Redis Configuration
    REDIS_HOST = os.getenv("REDIS_HOST", "localhost")
    REDIS_PORT = int(os.getenv("REDIS_PORT", "6379"))
    REDIS_DB = int(os.getenv("REDIS_DB", "0"))
    
    # API Keys
    OPENAI_API_KEY = os.getenv("OPENAI_API_KEY", "")

# Legacy constants for backward compatibility
DOCKER_NETWORK = Config.DOCKER_NETWORK
NASIKO_API_URL = Config.NASIKO_API_URL
KONG_GATEWAY_URL = Config.KONG_GATEWAY_URL
AGENT_REGISTRY_URL = Config.AGENT_REGISTRY_URL
AGENT_IMAGE_TAG = Config.AGENT_IMAGE_TAG

# Service Startup Configuration
NASIKO_APP_STARTUP_CHECK_INTERVAL = 30  # seconds
NASIKO_WEB_STARTUP_DELAY = 5   # seconds
KONG_STARTUP_DELAY = 10  # seconds
OLLAMA_STARTUP_DELAY = 15  # seconds

# Agent Configuration
AGENTS_DIRECTORY = "agents"
CONTAINER_HEALTH_TIMEOUT = 60  # seconds

# Docker Compose Files
NASIKO_APP_COMPOSE_FILE = "app/docker-compose.app.yaml"
NASIKO_WEB_COMPOSE_FILE = "web/docker-compose.yml"
KONG_COMPOSE_FILE = "kong/docker-compose.yml"
OLLAMA_COMPOSE_FILE = "models/ollama/docker-compose.yml"
