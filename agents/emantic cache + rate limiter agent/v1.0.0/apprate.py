import time
from collections import deque

from app.store import rate_windows, rate_limits, queue_depths, DEFAULT_RATE_LIMIT_RPM

WINDOW_SECONDS = 60


# ── Limit helpers ──────────────────────────────────────────────────
def set_limit(agent: str, rpm: int):
    rate_limits[agent] = rpm


def get_limit(agent: str) -> int:
    return rate_limits.get(agent, DEFAULT_RATE_LIMIT_RPM)


# ── Sliding-window check ──────────────────────────────────────────
def check_and_consume(agent: str) -> tuple[bool, dict]:
    """
    Check whether the agent is under its rate limit.
    If allowed: record the timestamp and return (True, info).
    If denied:  return (False, info) with queue position + wait estimate.
    """
    now = time.time()
    limit = get_limit(agent)

    if agent not in rate_windows:
        rate_windows[agent] = deque()

    window = rate_windows[agent]

    # Evict timestamps older than the sliding window
    while window and window[0] < now - WINDOW_SECONDS:
        window.popleft()

    current_count = len(window)

    if current_count < limit:
        window.append(now)
        return True, {
            "allowed": True,
            "current_count": current_count + 1,
            "limit": limit,
            "remaining": limit - current_count - 1,
        }

    # Over limit — compute wait info
    oldest = window[0]
    wait_seconds = max(0.0, oldest + WINDOW_SECONDS - now)
    wait_ms = int(wait_seconds * 1000)
    queue_depths[agent] = queue_depths.get(agent, 0) + 1

    return False, {
        "allowed": False,
        "current_count": current_count,
        "limit": limit,
        "queue_position": queue_depths.get(agent, 1),
        "estimated_wait_ms": wait_ms,
        "retry_after_seconds": int(wait_seconds) + 1,
    }


# ── Stats ──────────────────────────────────────────────────────────
def get_stats(agent: str | None = None) -> dict:
    now = time.time()

    def ag_stats(ag: str) -> dict:
        window = rate_windows.get(ag, deque())
        active = sum(1 for ts in window if ts >= now - WINDOW_SECONDS)
        limit = get_limit(ag)
        return {
            "agent": ag,
            "active_requests_in_window": active,
            "limit_rpm": limit,
            "remaining": max(0, limit - active),
            "queue_depth": queue_depths.get(ag, 0),
            "utilization_pct": round(active / limit * 100, 1) if limit > 0 else 0,
        }

    if agent:
        return ag_stats(agent)

    all_agents = set(list(rate_windows.keys()) + list(rate_limits.keys()))
    return {ag: ag_stats(ag) for ag in all_agents}
