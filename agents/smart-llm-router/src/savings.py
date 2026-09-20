"""Savings math vs GPT-4o baseline — mirrors the DronaHQ JS transformer."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from session_store import STORE, SessionTotals


@dataclass
class Savings:
    baseline_usd: str
    actual_usd: str
    saved_usd: str
    pct_saved: str
    saved_usd_f: float
    baseline_usd_f: float
    actual_usd_f: float


def compute_savings(usage: dict[str, Any]) -> Savings:
    prompt = float(usage.get("prompt_tokens") or 0)
    completion = float(usage.get("completion_tokens") or 0)
    actual = float(usage.get("cost_usd") or 0.0)
    # GPT-4o baseline rates from the handoff ($ / 1k tokens).
    baseline = (prompt * 0.0025 + completion * 0.01) / 1000.0
    saved = max(0.0, baseline - actual)
    pct = "0.0%" if baseline <= 0 else f"{(saved / baseline) * 100:.1f}%"
    return Savings(
        baseline_usd=f"{baseline:.6f}",
        actual_usd=f"{actual:.6f}",
        saved_usd=f"{saved:.6f}",
        pct_saved=pct,
        saved_usd_f=saved,
        baseline_usd_f=baseline,
        actual_usd_f=actual,
    )


def apply_session(session_id: str, savings: Savings, cache_hit: bool) -> SessionTotals:
    return STORE.record(session_id, savings.saved_usd_f, cache_hit)
