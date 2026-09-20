"""Shared deterministic cadence checks for financial event series."""

from __future__ import annotations

import statistics
from typing import Sequence


def interval_regularity(intervals: Sequence[int | float]) -> tuple[bool, float | None]:
    """Return whether observed gaps are regular and their median cadence.

    The tolerance is shared by recurrence classification and category-level
    variable-expense projection: ordinary calendar variation is allowed up to
    three days or 10% of the median interval, whichever is larger.
    """
    if not intervals:
        return False, None
    numeric = [float(interval) for interval in intervals]
    median_interval = float(statistics.median(numeric))
    tolerance = max(3.0, median_interval * 0.10)
    regular = max(numeric) - min(numeric) <= tolerance
    return regular, median_interval