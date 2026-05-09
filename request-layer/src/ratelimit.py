import json
import time
from typing import Tuple

import numpy as np
import redis.asyncio as redis

from . import stats


async def is_rate_limited(
    redis_client: redis.Redis,
    agent_name: str,
    path: str,
    request_body: bytes,
    request_json: dict,
) -> Tuple[bool, int]:
    """
    Checks if a request should be rate-limited. Implements adaptive limiting
    and queuing.

    Returns:
        A tuple of (is_limited, queue_position).
        - (False, 0) if the request is allowed.
        - (True, N) if the request is queued at position N.
        - (True, -1) if the request is rejected because the queue is full.
    """
    current_time = int(time.time())

    # --- Adaptive Limit Calculation ---
    # Get request counts for the last 10 seconds
    timestamps = range(current_time - 10, current_time)
    keys = [f"rl:{agent_name}:{ts}" for ts in timestamps]
    counts = await redis_client.mget(keys)

    # Coerce None to 0 and convert to int
    y_values = np.array([int(c) if c is not None else 0 for c in counts], dtype=np.float64)
    x_values = np.arange(len(y_values), dtype=np.float64)

    # Calculate linear regression slope (velocity)
    slope = 0.0
    if len(y_values) > 1:
        x_mean = x_values.mean()
        y_mean = y_values.mean()
        denominator = np.sum((x_values - x_mean) ** 2)
        if denominator > 0:
            slope = float(np.sum((x_values - x_mean) * (y_values - y_mean)) / denominator)
    stats.stats["per_agent_velocity"][agent_name] = slope

    # Adjust rate limit based on velocity
    base_limit = stats.stats["per_agent_rate_limit"][agent_name]
    if slope > 0.5:  # Traffic is accelerating
        effective_limit = max(1.0, base_limit * 0.7)  # Tighten by 30%
        if slope > 2.0:
            stats.record_proactive_tightening(agent_name, "traffic accelerating")
    elif slope < -0.2:  # Traffic is calming
        effective_limit = min(100.0, base_limit * 1.3)  # Loosen by 30%
    else:
        effective_limit = base_limit

    # --- Token Counting ---
    key = f"rl:{agent_name}:{current_time}"

    async with redis_client.pipeline() as pipe:
        pipe.incr(key)
        pipe.expire(key, 10)  # Keep for velocity window
        current_count, _ = await pipe.execute()

    if current_count > effective_limit:
        # Limit exceeded, try to queue
        # Store the full request context so queue_worker can replay it
        queue_entry = json.dumps({
            "agent_name": agent_name,
            "path": path,
            "body": request_json,
            "queued_at": time.time(),
        })
        queue_key = f"queue:{agent_name}"
        queue_length = await redis_client.lpush(queue_key, queue_entry)

        if queue_length > 50:
            # Queue is full, reject — remove the item we just pushed
            await redis_client.lpop(queue_key)
            stats.stats["rejected"] += 1
            return True, -1
        else:
            stats.stats["queued"] += 1
            stats.stats["per_agent_queue_len"][agent_name] = queue_length
            return True, queue_length

    # Request is allowed
    stats.stats["per_agent_requests"][agent_name] += 1
    stats.record_request(agent_name)
    return False, 0


async def update_rate_limit(agent_name: str, new_limit: float):
    """Updates the rate limit for a specific agent."""
    stats.stats["per_agent_rate_limit"][agent_name] = float(new_limit)
