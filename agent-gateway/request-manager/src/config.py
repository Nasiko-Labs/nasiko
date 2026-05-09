from pydantic_settings import BaseSettings


class Settings(BaseSettings):
    REDIS_URL: str = "redis://redis:6379/0"

    DEFAULT_RATE_LIMIT_RPM: int = 60
    RATE_LIMIT_WINDOW_SECONDS: int = 60

    QUEUE_MAX_DEPTH: int = 100
    QUEUE_TIMEOUT_SECONDS: float = 30.0

    DEFAULT_CACHE_TTL_SECONDS: int = 3600

    UPSTREAM_CONNECT_TIMEOUT: float = 10.0
    UPSTREAM_READ_TIMEOUT: float = 300.0

    LOG_LEVEL: str = "INFO"

    OTEL_EXPORTER_OTLP_ENDPOINT: str = "http://phoenix-observability:4317"
    OTEL_SERVICE_NAME: str = "nasiko-request-manager"

    model_config = {
        "env_file": ".env",
        "env_file_encoding": "utf-8",
        "case_sensitive": False,
    }


settings = Settings()
