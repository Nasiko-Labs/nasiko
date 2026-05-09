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
    MINIMAX_API_KEY: Optional[str] = None
    MINIMAX_BASE_URL: str = "https://api.minimax.io/v1"
    OLLAMA_SERVER: str = "http://ollama:11434"

    # LLM Provider selection for the router
    # Supported values: "openai", "openrouter", "minimax"
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

    # -----------------------------------------------------------------------
    # Response Cache settings
    # -----------------------------------------------------------------------
    # Redis URL for the response cache.  Leave empty to use the in-process LRU.
    # Example: redis://nasiko-cache-redis:6379/1
    CACHE_REDIS_URL: Optional[str] = None
    # How long (seconds) a cached response is considered fresh
    CACHE_TTL_SECONDS: int = 300          # 5 minutes
    # Maximum entries in the in-process LRU fallback cache
    CACHE_MAX_SIZE: int = 512

    # -----------------------------------------------------------------------
    # Rate Limiter settings (per-agent token bucket)
    # -----------------------------------------------------------------------
    # Sustained request rate allowed per agent (tokens/second)
    RATE_LIMIT_REQUESTS_PER_SECOND: float = 5.0
    # Maximum burst size (tokens in a full bucket)
    RATE_LIMIT_BURST_CAPACITY: int = 10
    # Maximum number of requests that can wait in the queue per agent
    RATE_LIMIT_QUEUE_SIZE: int = 20
    # Seconds a queued request will wait before being rejected
    RATE_LIMIT_QUEUE_TIMEOUT: float = 30.0

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
