from pydantic_settings import BaseSettings
from pydantic import field_validator


class Settings(BaseSettings):
    REDIS_URL: str = "redis://redis:6379"
    KONG_ADMIN_URL: str = "http://kong-gateway:8001"
    AGENTS_NETWORK: str = "agents-net"

    HOST: str = "0.0.0.0"
    PORT: int = 8090
    LOG_LEVEL: str = "INFO"

    REQUEST_TIMEOUT: float = 120.0

    CACHE_SIMILARITY_THRESHOLD: float = 0.92
    DETECTOR_INTERVAL_SECONDS: int = 10

    @field_validator("LOG_LEVEL")
    @classmethod
    def validate_log_level(cls, v: str) -> str:
        valid = ["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"]
        if v.upper() not in valid:
            raise ValueError(f"LOG_LEVEL must be one of {valid}")
        return v.upper()

    model_config = {
        "env_file": [".env"],
        "env_file_encoding": "utf-8",
        "case_sensitive": False,
    }


settings = Settings()
