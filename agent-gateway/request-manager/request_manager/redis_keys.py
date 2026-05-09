def targets_index() -> str:
    return "request-manager:targets"


def target(agent_id: str) -> str:
    return f"request-manager:targets:{agent_id}"


def cache_entry(cache_key: str) -> str:
    return f"request-manager:cache:{cache_key}"


def singleflight_lock(cache_key: str) -> str:
    return f"request-manager:singleflight:{cache_key}"


def singleflight_ready(cache_key: str) -> str:
    return f"request-manager:singleflight:ready:{cache_key}"


def limits(agent_id: str) -> str:
    return f"request-manager:limits:{agent_id}"


def active(agent_id: str) -> str:
    return f"request-manager:active:{agent_id}"


def active_global() -> str:
    return "request-manager:active:global"


def queue(agent_id: str) -> str:
    return f"request-manager:queue:{agent_id}"


def bucket(agent_id: str) -> str:
    return f"request-manager:bucket:{agent_id}"


def circuit(agent_id: str) -> str:
    return f"request-manager:circuit:{agent_id}"


def outcomes(agent_id: str) -> str:
    return f"request-manager:outcomes:{agent_id}"


def metrics_global() -> str:
    return "request-manager:metrics:global"


def metrics_agent(agent_id: str) -> str:
    return f"request-manager:metrics:{agent_id}"


def latency(agent_id: str) -> str:
    return f"request-manager:latency:{agent_id}"


def queue_wait(agent_id: str) -> str:
    return f"request-manager:queue-wait:{agent_id}"
