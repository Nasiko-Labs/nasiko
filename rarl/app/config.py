import json
from functools import lru_cache
from pydantic import field_validator
from pydantic_settings import BaseSettings


class Settings(BaseSettings):
    redis_url: str = "redis://redis:6379/2"
    agent_base_urls: dict[str, str] = {}
    default_ttl_seconds: int = 300
    default_rps: float = 10.0
    default_burst: int = 20
    default_max_inflight: int = 4
    default_max_queue: int = 100
    target_p95_latency: float = 1.0
    log_level: str = "INFO"
    adaptive_enabled: bool = False

    @field_validator("agent_base_urls", mode="before")
    @classmethod
    def parse_agent_base_urls(cls, v: str | dict) -> dict:
        if isinstance(v, str):
            return json.loads(v)
        return v

    model_config = {"env_file": ".env", "extra": "ignore"}


@lru_cache
def get_settings() -> Settings:
    return Settings()
