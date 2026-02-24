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
    OLLAMA_SERVER: str = "http://ollama:11434"

    # Vector store settings
    VECTOR_STORE_CACHE_TTL: int = 3600
    EMBEDDING_MODEL: str = "text-embedding-3-small"
    RERANKING_EMBEDDING_MODEL: str = "text-embedding-3-small"

    # Request settings
    MAX_FILE_SIZE: int = 1073741824  # 1GB
    REQUEST_TIMEOUT: float = 60.0
    MAX_CONCURRENT_REQUESTS: int = 10

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
