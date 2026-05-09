from functools import lru_cache

from pydantic import Field
from pydantic_settings import BaseSettings, SettingsConfigDict


class RequestManagerSettings(BaseSettings):
    model_config = SettingsConfigDict(
        env_prefix="REQUEST_MANAGER_",
        extra="ignore",
        populate_by_name=True,
    )

    redis_url: str = Field(default="redis://redis:6379", validation_alias="REDIS_URL")
    service_name: str = "nasiko-request-manager"
    cache_ttl_seconds: int = 600
    max_concurrency_per_agent: int = 2
    sustained_rps_per_agent: float = 5.0
    burst_capacity_per_agent: int = 10
    max_queue_depth_per_agent: int = 20
    max_queue_wait_ms: int = 10_000
    upstream_timeout_seconds: float = 45.0
    global_active_cap: int = 50
    circuit_window_size: int = 20
    circuit_min_failures: int = 5
    circuit_failure_ratio: float = 0.5
    circuit_open_seconds: int = 30
    singleflight_wait_ms: int = 10_000
    redis_timeout_seconds: float = 1.0
    admin_token: str | None = None


@lru_cache
def get_settings() -> RequestManagerSettings:
    return RequestManagerSettings()
