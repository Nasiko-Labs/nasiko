"""
Orchestrator Configuration
Contains all constants and configuration settings.
"""

import os


class Config:
    # Docker Configuration
    DOCKER_NETWORK = os.getenv("DOCKER_NETWORK", "nasiko-network")
    AGENTS_NETWORK = os.getenv("AGENTS_NETWORK", "agents-net")
    APP_NETWORK = os.getenv("APP_NETWORK", "app-network")
    NASIKO_API_URL = os.getenv("NASIKO_API_URL", "http://localhost:8000")
    KONG_GATEWAY_URL = os.getenv("KONG_GATEWAY_URL", "http://localhost:9100")

    # Agent Registry Configuration (for pre-built images)
    AGENT_REGISTRY_URL = os.getenv("AGENT_REGISTRY_URL", "docker.io")
    AGENT_IMAGE_TAG = os.getenv("AGENT_IMAGE_TAG", "latest")

    # Redis Configuration
    REDIS_HOST = os.getenv("REDIS_HOST", "localhost")
    REDIS_PORT = int(os.getenv("REDIS_PORT", "6379"))
    REDIS_DB = int(os.getenv("REDIS_DB", "0"))

    # API Keys (legacy direct-provider path — kept for backward compatibility)
    OPENAI_API_KEY = os.getenv("OPENAI_API_KEY", "")
    OPENROUTER_API_KEY = os.getenv("OPENROUTER_API_KEY", "")
    MINIMAX_API_KEY = os.getenv("MINIMAX_API_KEY", "")
    MINIMAX_BASE_URL = os.getenv("MINIMAX_BASE_URL", "https://api.minimax.io/v1")

    # LLM Gateway (Track 2) — platform-managed LiteLLM proxy.
    # When enabled, agents receive LITELLM_BASE_URL + a virtual key at
    # deploy time instead of raw provider credentials. Provider swap and
    # key rotation then happen in gateway config, not in agent code.
    LITELLM_ENABLED = os.getenv("LITELLM_ENABLED", "true").lower() in (
        "1", "true", "yes", "on",
    )
    LITELLM_BASE_URL = os.getenv("LITELLM_BASE_URL", "http://litellm:4000")
    LITELLM_VIRTUAL_KEY = os.getenv("LITELLM_VIRTUAL_KEY", "")
    LITELLM_DEFAULT_MODEL = os.getenv("LITELLM_DEFAULT_MODEL", "gpt-4o-mini")


# Legacy constants for backward compatibility
DOCKER_NETWORK = Config.DOCKER_NETWORK
AGENTS_NETWORK = Config.AGENTS_NETWORK
APP_NETWORK = Config.APP_NETWORK
NASIKO_API_URL = Config.NASIKO_API_URL
KONG_GATEWAY_URL = Config.KONG_GATEWAY_URL
AGENT_REGISTRY_URL = Config.AGENT_REGISTRY_URL
AGENT_IMAGE_TAG = Config.AGENT_IMAGE_TAG

# Service Startup Configuration
NASIKO_APP_STARTUP_CHECK_INTERVAL = 30  # seconds
NASIKO_WEB_STARTUP_DELAY = 5  # seconds
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
