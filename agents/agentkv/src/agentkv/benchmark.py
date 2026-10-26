"""Observed Pareto points with explicit comparability and quality constraints."""
from __future__ import annotations
from dataclasses import dataclass
from decimal import Decimal
import math

from agentkv.accounting import RunCost


@dataclass(frozen=True)
class Measurement:
    run_id: str
    method: str
    cohort: str  # Hash of model, precision, hardware, workload, offered load and cache regime.
    p95_seconds: float
    duration_seconds: float
    cost: RunCost
    cost_basis: str = "usage_estimate"

    def __post_init__(self):
        if not self.run_id or not self.cohort or self.method not in {"A", "B", "C", "D", "E"}:
            raise ValueError("run, cohort and A–E method are required")
        if any(not math.isfinite(x) or x <= 0 for x in (self.p95_seconds, self.duration_seconds)):
            raise ValueError("latency and duration must be finite and positive")
        if self.cost_basis not in {"usage_estimate", "reconciled_bill"}:
            raise ValueError("projections are not observed benchmark measurements")

    @property
    def dollars_per_thousand(self):
        cost = self.cost.cost_per_success
        return cost * Decimal(1000) if cost is not None else None

    @property
    def success_rate(self):
        total = self.cost.successful_workflows + self.cost.failed_workflows + self.cost.pending_workflows
        return self.cost.successful_workflows / total if total else 0.0


def frontier(points: list[Measurement], *, minimum_success_rate: float) -> list[Measurement]:
    """Return observed non-dominated runs, not an interpolated or universal optimum."""
    if not math.isfinite(minimum_success_rate) or not 0 <= minimum_success_rate <= 1:
        raise ValueError("quality threshold must be in [0,1]")
    if len({p.run_id for p in points}) != len(points):
        raise ValueError("duplicate run IDs")
    if len({(p.cohort, p.cost_basis) for p in points}) > 1:
        raise ValueError("incomparable cohorts or cost bases require separate panels")
    eligible = [p for p in points if p.dollars_per_thousand is not None
                and p.cost.pending_workflows == 0 and p.success_rate >= minimum_success_rate]
    return sorted([p for p in eligible if not any(
        q.p95_seconds <= p.p95_seconds and q.dollars_per_thousand <= p.dollars_per_thousand
        and (q.p95_seconds < p.p95_seconds or q.dollars_per_thousand < p.dollars_per_thousand)
        for q in eligible)], key=lambda p: (p.p95_seconds, p.dollars_per_thousand, p.run_id))
