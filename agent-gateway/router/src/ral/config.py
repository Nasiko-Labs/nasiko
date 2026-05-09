"""
RAL Configuration
=================
All tunable knobs for the Resilient Agent Request Layer, sourced from
environment variables. Inherits the same pydantic_settings pattern used
throughout the router service.
"""

from __future__ import annotations

import json
from typing import Dict, Any, Optional
from pydantic import field_validator
from pydantic_settings import BaseSettings


class RalConfig(BaseSettings):
    """Configuration for the Resilient Agent Request Layer."""

    # --------------- Cache -----------------------------------------------
    # How long (seconds) a cached response is considered fresh.
    # Global semantic deduplication: cache key is (query, model, provider, params)
    # — session_id is intentionally excluded to maximise the hit-rate across users.
    RAL_CACHE_TTL: int = 300

    # --------------- Rate Limiting ----------------------------------------
    # Token-bucket algorithm applied uniformly to every agent (all agents share
    # the same global defaults; no per-agent overrides are needed right now).
    RAL_RATE_LIMIT_RPS: float = 10.0       # sustained tokens/second
    RAL_RATE_LIMIT_BURST: int = 20         # max burst above the sustained rate

    # Maximum number of requests that can be active against a single agent
    # at the same time (concurrent request cap).
    RAL_MAX_CONCURRENT: int = 5

    # --------------- Queue -------------------------------------------------
    RAL_MAX_QUEUE_SIZE: int = 100          # hard cap on backlog depth
    RAL_QUEUE_TIMEOUT: float = 30.0        # seconds a queued request may wait
    RAL_MAX_RETRIES: int = 3               # retries on transient 5xx failures
    RAL_RETRY_DELAY: float = 1.0           # base delay (seconds) between retries

    # --------------- Metrics -----------------------------------------------
    # How many seconds of request-log entries to keep in Redis.
    RAL_METRICS_RETENTION: int = 3600      # 1 hour

    # Maximum number of recent request log entries stored in Redis.
    RAL_LOG_MAX_ENTRIES: int = 1000

    # Rolling window (seconds) used to compute the requests/sec figure.
    RAL_RPS_WINDOW: int = 10

    # --------------- Redis namespace ----------------------------------------
    RAL_REDIS_PREFIX: str = "ral"

    # --------------- Misc ---------------------------------------------------
    # Redis connection URL — inherited from the broader RouterConfig env.
    REDIS_URL: Optional[str] = None
    REDIS_HOST: str = "localhost"
    REDIS_PORT: int = 6379
    REDIS_DB: int = 0

    @property
    def redis_dsn(self) -> str:
        if self.REDIS_URL:
            return self.REDIS_URL
        return f"redis://{self.REDIS_HOST}:{self.REDIS_PORT}/{self.REDIS_DB}"

    model_config = {
        "env_file": [".env", "router/.env"],
        "env_file_encoding": "utf-8",
        "case_sensitive": False,
        "extra": "ignore",
    }


# Singleton — import this everywhere inside the RAL package.
ral_settings = RalConfig()
