"""Runtime settings (pydantic-settings)."""
import logging
from functools import lru_cache

from pydantic import Field
from pydantic_settings import BaseSettings, SettingsConfigDict

logger = logging.getLogger(__name__)


class Settings(BaseSettings):
    """Runtime settings for the Request layer service.

    All values can be overridden by environment variables prefixed with the
    field name (e.g. ``REQUEST_LAYER_PORT``). Defaults are tuned for the local
    docker-compose stack.
    """

    model_config = SettingsConfigDict(
        env_file=(".nasiko-local.env", ".env"),
        env_file_encoding="utf-8",
        extra="ignore",
        case_sensitive=False,
    )

    # Service binding
    request_layer_host: str = Field(default="0.0.0.0")
    request_layer_port: int = Field(default=8090)
    request_layer_log_level: str = Field(default="INFO")

    # Redis (the request layer owns its own redis-stack instance for vector
    # search; see docker-compose.local.yml for the ``request-layer-redis``
    # service).
    request_layer_redis_url: str = Field(
        default="redis://request-layer-redis:6379/0"
    )

    # Embedding model
    request_layer_embedding_model: str = Field(
        default="sentence-transformers/all-MiniLM-L6-v2"
    )
    request_layer_embedding_dim: int = Field(default=384)

    # Cache thresholds
    request_layer_semantic_threshold: float = Field(default=0.95)
    request_layer_router_cache_threshold: float = Field(default=0.97)
    request_layer_router_cache_enabled: bool = Field(default=False)

    # Default policy values (overridden per-agent by AgentCard inference)
    request_layer_default_ttl_seconds: int = Field(default=600)
    request_layer_default_rps: int = Field(default=50)
    request_layer_default_cost_cap_usd_per_min: float = Field(default=1.0)

    # Coalescer
    request_layer_coalesce_wait_seconds: int = Field(default=30)

    # Nasiko integration
    request_layer_nasiko_registry_url: str = Field(
        default="http://nasiko-backend:8000"
    )
    request_layer_kong_admin_url: str = Field(default="http://kong-gateway:8001")
    request_layer_kong_proxy_internal: str = Field(default="http://kong-gateway:8000")
    request_layer_registry_poll_seconds: int = Field(default=60)

    # Phoenix / OTel — Request layer reuses ``app.utils.observability.tracing_utils``.
    request_layer_phoenix_endpoint: str = Field(
        default="http://phoenix-observability:4317"
    )
    request_layer_phoenix_project: str = Field(default="nasiko-request-layer")
    request_layer_tracing_enabled: bool = Field(default=True)

    # Admin / forward
    request_layer_forward_timeout_seconds: float = Field(default=30.0)
    request_layer_forward_max_connections: int = Field(default=100)
    request_layer_admin_stream_max_events: int = Field(default=200)


@lru_cache(maxsize=1)
def get_settings() -> Settings:
    """Return a cached settings singleton."""

    settings = Settings()
    logger.debug(
        "loaded request_layer settings (port=%s redis=%s router_cache_enabled=%s)",
        settings.request_layer_port,
        settings.request_layer_redis_url,
        settings.request_layer_router_cache_enabled,
    )
    return settings
