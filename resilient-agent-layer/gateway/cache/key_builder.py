import hashlib
import json
from typing import Any, Dict


def build_cache_key(agent_id: str, payload: Any) -> str:
    """
    Build a deterministic cache key from agent_id + normalized request payload.
    Uses SHA-256 to produce a fixed-length, collision-resistant key.
    """
    if isinstance(payload, dict):
        # Sort keys for determinism regardless of insertion order
        normalized = json.dumps(payload, sort_keys=True, ensure_ascii=False)
    elif isinstance(payload, str):
        normalized = payload
    else:
        normalized = str(payload)

    raw = f"{agent_id}::{normalized}"
    digest = hashlib.sha256(raw.encode("utf-8")).hexdigest()
    return f"cache:{agent_id}:{digest}"
