import os
import json
from typing import Dict, Any

# Redis
REDIS_URL = os.getenv("REDIS_URL", "redis://localhost:6379")

# Cache defaults
DEFAULT_CACHE_TTL = int(os.getenv("DEFAULT_CACHE_TTL", "60"))  # seconds

# Rate limiter defaults
DEFAULT_RATE_LIMIT_RPS = float(os.getenv("DEFAULT_RATE_LIMIT_RPS", "10"))
DEFAULT_BURST_CAPACITY = int(os.getenv("DEFAULT_BURST_CAPACITY", "20"))

# Queue defaults
DEFAULT_QUEUE_MAX_SIZE = int(os.getenv("DEFAULT_QUEUE_MAX_SIZE", "100"))
DEFAULT_QUEUE_TIMEOUT_MS = int(os.getenv("DEFAULT_QUEUE_TIMEOUT_MS", "5000"))

# Agent fleet config path
AGENT_CONFIG_PATH = os.getenv("AGENT_FLEET_CONFIG", "./agents.json")

# Admin
ADMIN_API_KEY = os.getenv("ADMIN_API_KEY", "")  # empty = open for demo


def load_agent_fleet() -> Dict[str, Any]:
    """Load agent fleet configuration from JSON file or env."""
    if os.path.exists(AGENT_CONFIG_PATH):
        with open(AGENT_CONFIG_PATH) as f:
            return json.load(f)
    # Fallback: discover from env
    return {
        "agent-a": {"base_url": os.getenv("AGENT_A_URL", "http://agent-a:8001"), "cache_ttl": 60, "rate_limit_rps": 10, "burst": 20, "queue_max": 50},
        "agent-b": {"base_url": os.getenv("AGENT_B_URL", "http://agent-b:8002"), "cache_ttl": 30, "rate_limit_rps": 5,  "burst": 10, "queue_max": 30},
        "agent-slow": {"base_url": os.getenv("AGENT_SLOW_URL", "http://agent-slow:8003"), "cache_ttl": 120, "rate_limit_rps": 2, "burst": 5, "queue_max": 20},
    }
