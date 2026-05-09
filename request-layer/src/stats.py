from collections import defaultdict
import time

# In-memory store for statistics.
stats = {
    "exact_hits": 0,
    "semantic_hits": 0,
    "misses": 0,
    "queued": 0,
    "rejected": 0,
    "per_agent_requests": defaultdict(int),
    "per_agent_queue_len": defaultdict(int),
    "per_agent_velocity": defaultdict(float),
    "per_agent_rate_limit": defaultdict(lambda: 10.0),
    "per_agent_exact_hits": defaultdict(int),
    "per_agent_semantic_hits": defaultdict(int),
    "prevented_overloads": 0,
    "proactive_tightening_events": [],
}

# Track known agents
known_agents = set()

# Rolling window of per-agent request counts for the chart
request_history = defaultdict(list)

_start_time = time.time()


def record_request(agent_name: str):
    """Record a request timestamp for the rolling chart."""
    known_agents.add(agent_name)
    now = time.time()
    request_history[agent_name].append(now)
    cutoff = now - 60
    request_history[agent_name] = [
        t for t in request_history[agent_name] if t > cutoff
    ]


def record_proactive_tightening(agent_name: str, reason: str):
    """Record a proactive rate limit tightening event."""
    stats["prevented_overloads"] += 1
    event = {
        "agent": agent_name,
        "time": time.strftime("%H:%M:%S"),
        "reason": reason,
    }
    stats["proactive_tightening_events"].append(event)
    # Keep last 50 events
    if len(stats["proactive_tightening_events"]) > 50:
        stats["proactive_tightening_events"] = stats["proactive_tightening_events"][-50:]


def get_stats():
    """Returns statistics in the expected dashboard format."""
    total_hits = stats["exact_hits"] + stats["semantic_hits"]
    total_lookups = total_hits + stats["misses"]
    hit_rate = round((total_hits / total_lookups) * 100, 1) if total_lookups > 0 else 0.0

    all_agents = list(
        known_agents
        | set(stats["per_agent_requests"].keys())
        | set(stats["per_agent_queue_len"].keys())
    )

    queue_lengths = {}
    velocity_slopes = {}
    rate_limits = {}

    for agent in all_agents:
        queue_lengths[agent] = stats["per_agent_queue_len"].get(agent, 0)
        velocity_slopes[agent] = round(stats["per_agent_velocity"].get(agent, 0.0), 2)
        rate_limits[agent] = round(stats["per_agent_rate_limit"][agent], 1)

    return {
        "cache": {
            "exact_hits": stats["exact_hits"],
            "semantic_hits": stats["semantic_hits"],
            "misses": stats["misses"],
            "hit_rate_percent": hit_rate,
        },
        "traffic": {
            "queued": stats["queued"],
            "rejected": stats["rejected"],
            "queue_lengths": queue_lengths,
            "velocity_slopes": velocity_slopes,
            "prevented_overloads": stats["prevented_overloads"],
            "proactive_tightening_events": list(stats["proactive_tightening_events"]),
        },
        "rate_limits": rate_limits,
        "agents": all_agents,
        "uptime_seconds": int(time.time() - _start_time),
    }


def get_cache_hit_rate():
    """Calculates the overall cache hit rate."""
    total_hits = stats["exact_hits"] + stats["semantic_hits"]
    total_lookups = total_hits + stats["misses"]
    return round((total_hits / total_lookups) * 100, 2) if total_lookups > 0 else 0.0
