"""Deterministic composition of reuse estimates into retention and warming actions.

Jev supplies probabilities. This module never rewrites prompts or executes tools.
"""
from agentkv.measurements import counter


def retention_score(repair_probability: float, avoided_ms: float, resident_bytes: float) -> float:
    if avoided_ms <= 0 or resident_bytes <= 0:
        raise ValueError("avoided recomputation and resident bytes must be positive")
    return (1 + repair_probability) * avoided_ms / resident_bytes


def engine_idle(metrics_text: str) -> bool | None:
    """None when the engine does not expose running/waiting gauges."""
    running = counter(metrics_text, "vllm:num_requests_running")
    waiting = counter(metrics_text, "vllm:num_requests_waiting")
    if running is None and waiting is None:
        return None
    return (running or 0) == 0 and (waiting or 0) == 0


def should_warm(*, method: str, warming_enabled: bool, idle: bool | None,
                repair_probability: float, reuse_prefix_probability: float,
                repair_skip: float = .7, reuse_floor: float = .5) -> bool:
    """Warm only an exactly reconstructable later prompt while the GPU is idle.

    High repair probability means the next LLM call is a continuation of the
    coder conversation, not the reviewer prefix, so warming the reviewer is waste.
    """
    if method not in {"C", "D", "E"} or not warming_enabled:
        return False
    if idle is not True:
        return False
    if repair_probability >= repair_skip:
        return False
    return reuse_prefix_probability >= reuse_floor


def check_support(task: dict, label: str, draft: str) -> dict:
    expected = task["label"].strip().lower()
    token = (label or "").strip().lower().split()[0] if (label or "").strip() else ""
    observed = token.strip(".:;!,")
    if observed != expected:
        return {"passed": False, "error": "wrong_label", "expected": expected, "observed": observed}
    missing = [phrase for phrase in task["required_phrases"] if phrase.lower() not in draft.lower()]
    return {"passed": not missing, "missing": missing}
