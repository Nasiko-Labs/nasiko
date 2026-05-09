"""
Configuration for Sentinel Guard.
All values are overridable via environment variables.
"""

import os


class SentinelConfig:
    """Centralized configuration with env-var overrides."""

    # ── Redis ──────────────────────────────────────────────────────────────
    REDIS_HOST: str = os.getenv("REDIS_HOST", "localhost")
    REDIS_PORT: int = int(os.getenv("REDIS_PORT", "6379"))
    REDIS_DB: int = int(os.getenv("REDIS_DB", "0"))

    # ── Cache ──────────────────────────────────────────────────────────────
    CACHE_TTL_SECONDS: int = int(os.getenv("CACHE_TTL_SECONDS", "1800"))  # 30 min
    SIMILARITY_THRESHOLD: float = float(os.getenv("SIMILARITY_THRESHOLD", "0.92"))
    MAX_CACHE_SIZE_PER_AGENT: int = int(os.getenv("MAX_CACHE_SIZE_PER_AGENT", "1000"))

    # ── Rate Limiting ──────────────────────────────────────────────────────
    RATE_LIMIT_DEFAULT_RPM: int = int(os.getenv("RATE_LIMIT_DEFAULT_RPM", "60"))
    RATE_LIMIT_WINDOW_SECONDS: int = int(os.getenv("RATE_LIMIT_WINDOW_SECONDS", "60"))

    # ── Queue ──────────────────────────────────────────────────────────────
    MAX_QUEUE_DEPTH: int = int(os.getenv("MAX_QUEUE_DEPTH", "50"))
    QUEUE_ITEM_TIMEOUT_SECONDS: int = int(
        os.getenv("QUEUE_ITEM_TIMEOUT_SECONDS", "120")
    )

    # ── Agent Gateway ──────────────────────────────────────────────────────
    NASIKO_BASE_URL: str = os.getenv(
        "NASIKO_BASE_URL", "http://localhost:9100/agents"
    )

    # ── Embedding ──────────────────────────────────────────────────────────
    EMBEDDING_MODEL: str = os.getenv("EMBEDDING_MODEL", "all-MiniLM-L6-v2")

    # ── Dashboard ──────────────────────────────────────────────────────────
    DASHBOARD_SSE_INTERVAL: float = float(
        os.getenv("DASHBOARD_SSE_INTERVAL", "2.0")
    )


config = SentinelConfig()
