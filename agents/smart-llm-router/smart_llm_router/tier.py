"""Tier decision — mirrors the DronaHQ JS transformer node."""

from __future__ import annotations

from typing import Literal

TaskType = Literal["summarize", "translate", "reason", "code", "chat", "extract"]
Tier = Literal["cheap", "balanced", "premium"]

VALID_TASK_TYPES = frozenset(
    {"summarize", "translate", "reason", "code", "chat", "extract"}
)


def decide_tier(task_type: str, complexity: int) -> Tier:
    """Pick the cheapest viable tier from classifier output."""
    tt = task_type if task_type in VALID_TASK_TYPES else "chat"
    c = complexity if isinstance(complexity, int) else 3
    c = max(1, min(5, c))

    tier: Tier = "balanced"
    if c <= 2:
        tier = "cheap"
    elif tt == "reason" and c >= 4:
        tier = "premium"
    elif tt == "code" and c >= 3:
        tier = "premium"
    return tier
