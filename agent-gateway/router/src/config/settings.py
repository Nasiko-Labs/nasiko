"""
Router service configuration settings.
Centralized configuration handling with validation and environment-specific settings.
"""

from typing import Optional, List
from pydantic_settings import BaseSettings
from pydantic import field_validator


class RouterConfig(BaseSettings):
    """Router service configuration settings."""

    # Environment
    ENV: str = "development"

    # External API settings
    NASIKO_BACKEND: str = "http://nasiko-backend:8000/api/v1"
    OPENAI_API_KEY: Optional[str] = None
    OPENROUTER_API_KEY: Optional[str] = None
    NVIDIA_API_KEY: Optional[str] = None
    NVIDIA_BASE_URL: str = "https://integrate.api.nvidia.com/v1"
    MINIMAX_API_KEY: Optional[str] = None
    MINIMAX_BASE_URL: str = "https://api.minimax.io/v1"
    OLLAMA_SERVER: str = "http://ollama:11434"

    # LLM Provider selection for the router
    # Supported values: "openai", "openrouter", "nvidia", "minimax"
    ROUTER_LLM_PROVIDER: str = "openai"
    ROUTER_LLM_MODEL: str = "gpt-4o-mini"

    # Vector store settings
    VECTOR_STORE_CACHE_TTL: int = 3600
    EMBEDDING_PROVIDER: str = "openai"  # "openai" | "jina"
    EMBEDDING_MODEL: str = "text-embedding-3-small"
    RERANKING_EMBEDDING_MODEL: str = "text-embedding-3-small"
    JINA_API_KEY: Optional[str] = None
    JINA_EMBEDDING_MODEL: str = "jina-embeddings-v3"

    # Request settings
    MAX_FILE_SIZE: int = 1073741824  # 1GB
    REQUEST_TIMEOUT: float = 60.0
    MAX_CONCURRENT_REQUESTS: int = 10

    # Resilient request layer settings
    RESILIENCE_ENABLED: bool = True
    RESILIENCE_CACHE_TTL_SECONDS: float = 3600.0
    RESILIENCE_SEMANTIC_ENABLED: bool = False
    RESILIENCE_SEMANTIC_THRESHOLD: float = 0.92
    RESILIENCE_DEFAULT_AGENT_RPS: float = 5.0
    RESILIENCE_MIN_AGENT_RPS: float = 0.25
    RESILIENCE_BURST: int = 5
    RESILIENCE_MAX_QUEUE_DEPTH: int = 50
    RESILIENCE_MAX_QUEUE_WAIT_SECONDS: float = 10.0
    RESILIENCE_TARGET_LATENCY_SECONDS: float = 2.0
    RESILIENCE_ADMIN_API_KEY: Optional[str] = None
    REDIS_URL: Optional[str] = None

    # Server settings
    HOST: str = "0.0.0.0"
    PORT: int = 8000
    RELOAD: bool = True

    # CORS settings - as comma-separated string that gets parsed
    CORS_ORIGINS: str = "http://localhost:4000,http://127.0.0.1:4000"

    # Logging settings
    LOG_LEVEL: str = "INFO"

    @property
    def cors_origins_list(self) -> List[str]:
        """Parse CORS origins from comma-separated string."""
        return [
            origin.strip() for origin in self.CORS_ORIGINS.split(",") if origin.strip()
        ]

    @field_validator("NASIKO_BACKEND")
    @classmethod
    def validate_backend_url(cls, v):
        if not v.startswith("http"):
            raise ValueError("Nasiko backend URL must start with http or https")
        return v

    @field_validator("LOG_LEVEL")
    @classmethod
    def validate_log_level(cls, v):
        valid_levels = ["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"]
        if v.upper() not in valid_levels:
            raise ValueError(f"Log level must be one of: {valid_levels}")
        return v.upper()

    model_config = {
        "env_file": [".env", "router/.env", "kong/router/.env"],
        "env_file_encoding": "utf-8",
        "case_sensitive": False,
    }


# Global configuration instance
settings = RouterConfig()
