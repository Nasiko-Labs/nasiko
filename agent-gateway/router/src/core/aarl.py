# agent-gateway/router/src/core/aarl.py

import asyncio
import hashlib
import json
import time
from typing import Dict, Any, Optional

import httpx
import redis.asyncio as redis
from fastapi import APIRouter

# ============================================================
# CONFIG
# ============================================================

REDIS_URL = "redis://redis:6379"

CACHE_TTL_SECONDS = 300

# ============================================================
# REDIS
# ============================================================

redis_client = redis.from_url(
    REDIS_URL,
    decode_responses=True,
)

# ============================================================
# METRICS
# ============================================================

metrics = {
    "requests_total": 0,
    "cache_hits": 0,
    "cache_misses": 0,
    "dedupe_hits": 0,
    "agent_calls": 0,
    "errors": 0,
    "active_inflight_requests": 0,
}

# ============================================================
# INFLIGHT REQUESTS
# ============================================================

inflight_requests: Dict[str, asyncio.Future] = {}

# ============================================================
# TOKEN BUCKET RATE LIMITER
# ============================================================

class TokenBucket:

    def __init__(
        self,
        capacity: int,
        refill_rate: float,
    ):
        self.capacity = capacity
        self.tokens = capacity
        self.refill_rate = refill_rate
        self.last_refill = time.time()
        self.lock = asyncio.Lock()

    async def allow_request(self):

        async with self.lock:

            now = time.time()

            elapsed = now - self.last_refill

            refill_amount = elapsed * self.refill_rate

            self.tokens = min(
                self.capacity,
                self.tokens + refill_amount,
            )

            self.last_refill = now

            if self.tokens >= 1:
                self.tokens -= 1
                return True

            return False

# ============================================================
# AGENT LIMITS
# ============================================================

agent_limiters = {
    "translator": TokenBucket(
        capacity=20,
        refill_rate=10,
    ),
    "compliance-checker": TokenBucket(
        capacity=5,
        refill_rate=2,
    ),
}

# ============================================================
# CACHE KEY
# ============================================================

def build_cache_key(
    agent_url: str,
    payload: Dict[str, Any],
) -> str:

    normalized = json.dumps(
        {
            "agent_url": agent_url,
            "payload": payload,
        },
        sort_keys=True,
    )

    return hashlib.sha256(
        normalized.encode()
    ).hexdigest()

# ============================================================
# CACHE
# ============================================================

async def get_cached_response(
    cache_key: str,
) -> Optional[Dict]:

    cached = await redis_client.get(cache_key)

    if not cached:
        return None

    return json.loads(cached)

async def set_cached_response(
    cache_key: str,
    value: Dict[str, Any],
):

    await redis_client.set(
        cache_key,
        json.dumps(value),
        ex=CACHE_TTL_SECONDS,
    )

# ============================================================
# MAIN EXECUTION
# ============================================================

async def execute_request(
    agent_name: str,
    agent_url: str,
    payload: Dict[str, Any],
    headers: Dict[str, Any],
    timeout,
):

    metrics["requests_total"] += 1

    cache_key = build_cache_key(
        agent_url,
        payload,
    )

    # ========================================================
    # CACHE HIT
    # ========================================================

    cached = await get_cached_response(
        cache_key,
    )

    if cached:

        metrics["cache_hits"] += 1

        return {
            "aarl_source": "cache",
            "data": cached,
        }

    metrics["cache_misses"] += 1

    # ========================================================
    # DEDUPE
    # ========================================================

    if cache_key in inflight_requests:

        metrics["dedupe_hits"] += 1

        return await inflight_requests[cache_key]

    # ========================================================
    # FIRST REQUEST CREATES FUTURE
    # ========================================================

    future = asyncio.Future()

    inflight_requests[cache_key] = future

    metrics["active_inflight_requests"] = len(
        inflight_requests
    )

    try:

        # ====================================================
        # RATE LIMIT
        # ====================================================

        limiter = agent_limiters.get(agent_name)

        if limiter:

            allowed = await limiter.allow_request()

            if not allowed:

                response = {
                    "aarl_source": "rate_limited",
                    "error": "agent overloaded",
                }

                future.set_result(response)

                return response

        # ====================================================
        # ACTUAL AGENT CALL
        # ====================================================

        metrics["agent_calls"] += 1

        async with httpx.AsyncClient(
            timeout=timeout
        ) as client:

            response = await client.post(
                agent_url,
                json=payload,
                headers=headers,
            )

            response.raise_for_status()

            data = response.json()

        # ====================================================
        # STORE CACHE
        # ====================================================

        await set_cached_response(
            cache_key,
            data,
        )

        wrapped = {
            "aarl_source": "agent",
            "data": data,
        }

        future.set_result(wrapped)

        return wrapped

    except Exception as e:

        metrics["errors"] += 1

        future.set_exception(e)

        raise e

    finally:

        inflight_requests.pop(
            cache_key,
            None,
        )

        metrics["active_inflight_requests"] = len(
            inflight_requests
        )

# ============================================================
# METRICS ROUTER
# ============================================================

router = APIRouter()

@router.get("/aarl/metrics")
async def aarl_metrics():

    total = (
        metrics["cache_hits"]
        + metrics["cache_misses"]
    )

    cache_hit_rate = 0

    if total > 0:
        cache_hit_rate = (
            metrics["cache_hits"] / total
        ) * 100

    return {
        **metrics,
        "cache_hit_rate": round(
            cache_hit_rate,
            2,
        ),
        "inflight_keys": list(
            inflight_requests.keys()
        ),
    }