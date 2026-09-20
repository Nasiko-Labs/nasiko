"""Run a deterministic 90-day balance simulation without LLM calls.

The engine consumes the structured state emitted by ``state_reconstruction`` and
projects signed cash flows using only dates, amounts, statuses, recurrence
intervals, and the user's minimum balance. It ignores pending credits, failed or
cancelled events, and unrealized investment values. Optional plan payments are
added as debit flows so callers can test affordability without any model call.
"""

from __future__ import annotations

import math
from collections import defaultdict
from dataclasses import asdict, dataclass
from datetime import date, timedelta
from typing import Any, Iterable, Mapping, Sequence

from regularity import interval_regularity


@dataclass(frozen=True)
class CashFlow:
    """One dated, signed cash movement used by the deterministic simulator."""

    date: date
    amount: float
    event_id: str | None
    source: str
    description: str
    credit_before_debit: bool = False

    def as_dict(self) -> dict[str, Any]:
        """Return a JSON-friendly flow representation."""
        value = asdict(self)
        value["date"] = self.date.isoformat()
        return value


def _as_date(value: date | str) -> date:
    """Normalize an ISO date string or date object."""
    if isinstance(value, date):
        return value
    return date.fromisoformat(str(value)[:10])


def _event_projection_date(event: Mapping[str, Any]) -> date | None:
    """Choose the cash-settlement date for a projected event."""
    value = event.get("settlement_date") or event.get("event_date")
    if not value:
        return None
    try:
        return _as_date(value)
    except ValueError:
        return None


def _recurrence_anchor_date(event: Mapping[str, Any]) -> date | None:
    """Use scheduled event dates for recurrence projection anchors."""
    value = event.get("event_date") or event.get("settlement_date")
    if not value:
        return None
    try:
        return _as_date(value)
    except ValueError:
        return None


def _event_amount(event: Mapping[str, Any]) -> float | None:
    """Return a finite, non-negative event amount in home currency."""
    value = event.get("amount_home_currency")
    if value is None:
        value = event.get("amount")
    if value is None:
        return None
    try:
        amount = float(value)
    except (TypeError, ValueError):
        return None
    return amount if math.isfinite(amount) and amount >= 0 else None


def _cash_flow_allowed(event: Mapping[str, Any]) -> bool:
    """Apply the challenge's cash-state rules to one normalized event."""
    if event.get("cash_flow_eligible") is False:
        return False
    status = str(event.get("status") or "")
    if status in {"cancelled", "failed", "unrealized"}:
        return False
    # Pending credits are explicitly not spendable until settled. Pending debits
    # are reserved and therefore remain in the forecast.
    if status == "pending" and event.get("direction") == "credit":
        return False
    return True


def _event_key(event: Mapping[str, Any]) -> tuple[str, ...]:
    """Return the stable identity used to group an event series."""
    return (
        str(event.get("event_type", "")),
        str(event.get("direction", "")),
        str(event.get("category", "")),
        str(event.get("description", "")),
        str(event.get("currency", "")),
    )


def _change_by_event_id(state: Mapping[str, Any]) -> dict[str, Mapping[str, Any]]:
    """Index validated flexible-spending changes by event ID."""
    changes: dict[str, Mapping[str, Any]] = {}
    for change in state.get("spending_changes", []) or []:
        event_id = change.get("event_id")
        if event_id:
            changes[str(event_id)] = change
    return changes


def _series_amount(
    members: Sequence[Mapping[str, Any]], state: Mapping[str, Any]
) -> float | None:
    """Choose a conservative amount, honoring tested flexible-expense overrides."""
    changes = _change_by_event_id(state)
    overrides = [changes.get(str(member.get("event_id"))) for member in members]
    if any(change and change.get("action") == "stop" for change in overrides):
        return 0.0
    values: list[float] = []
    for member, change in zip(members, overrides):
        amount = (
            change.get("new_amount")
            if change and change.get("action") == "reduce_to"
            else _event_amount(member)
        )
        if amount is not None:
            values.append(float(amount))
    if not values:
        return None
    direction = str(members[0].get("direction") or "")
    return max(values) if direction == "debit" else min(values)


def _event_override_amount(
    event: Mapping[str, Any], state: Mapping[str, Any]
) -> float | None | object:
    """Return an override amount, ``None`` for stop, or a sentinel for unchanged."""
    unchanged = _event_override_amount.UNCHANGED
    change = _change_by_event_id(state).get(str(event.get("event_id")))
    if not change:
        return unchanged
    if change.get("action") == "stop":
        return None
    if change.get("action") == "reduce_to":
        return float(change["new_amount"])
    return unchanged


_event_override_amount.UNCHANGED = object()  # type: ignore[attr-defined]


def _append_event_flow(
    flows: list[CashFlow], event: Mapping[str, Any], event_date: date, amount: float | None = None
) -> None:
    """Append one non-zero signed event flow to the forecast."""
    value = _event_amount(event) if amount is None else amount
    if value is None or value == 0:
        return
    signed = -value if event.get("direction") == "debit" else value
    flows.append(
        CashFlow(
            date=event_date,
            amount=signed,
            event_id=str(event.get("event_id")) if event.get("event_id") else None,
            source="event",
            description=str(
                event.get("description") or event.get("event_type") or "financial event"
            ),
            credit_before_debit=(
                event.get("direction") == "credit"
                and event.get("status") in {"settled", "scheduled"}
            ),
        )
    )


def _explicit_occurrence_matches(
    state: Mapping[str, Any],
    series: Sequence[Mapping[str, Any]],
    projected_date: date,
    projected_amount: float,
    interval_days: int,
) -> bool:
    """Detect a supplied future occurrence for a projected obligation.

    A scheduled record may use a more specific description (for example,
    ``Next confirmed salary``) than the historical recurring series. Match on
    the stable cash-flow identity and amount, then allow a small date window to
    absorb month-length cadence drift. This prevents one obligation from being
    credited once as an explicit event and again as a recurrence projection.
    """
    if not series:
        return False
    exemplar = series[0]
    identity = (
        exemplar.get("event_type"),
        exemplar.get("direction"),
        exemplar.get("category"),
        exemplar.get("currency"),
    )
    date_tolerance = max(3, int(round(float(interval_days or 0) * 0.10)))
    seen_ids: set[str] = set()
    candidates = list(state.get("events", []) or []) + list(state.get("one_time_events", []) or [])
    for event in candidates:
        event_id = str(event.get("event_id") or "")
        if event_id and event_id in seen_ids:
            continue
        if event_id:
            seen_ids.add(event_id)
        if event.get("is_recurring"):
            continue
        if str(event.get("status") or "") != "scheduled":
            continue
        event_identity = (
            event.get("event_type"),
            event.get("direction"),
            event.get("category"),
            event.get("currency"),
        )
        if event_identity != identity:
            continue
        amount = _event_amount(event)
        event_date = _event_projection_date(event)
        if amount is None or event_date is None:
            continue
        if abs(amount - projected_amount) > max(0.01, abs(projected_amount) * 0.001):
            continue
        if abs((event_date - projected_date).days) <= date_tolerance:
            return True
    return False


def _recurring_flows(
    state: Mapping[str, Any], start: date, end: date
) -> list[CashFlow]:
    """Project each reconstructed recurring series from its observed interval."""
    members: defaultdict[tuple[str, ...], list[Mapping[str, Any]]] = defaultdict(list)
    for field in ("confirmed_recurring_income", "confirmed_recurring_expenses"):
        for event in state.get(field, []) or []:
            if _cash_flow_allowed(event):
                members[_event_key(event)].append(event)

    flows: list[CashFlow] = []
    for series in members.values():
        if not series:
            continue
        interval_value = next(
            (
                event.get("recurrence_interval_days")
                for event in series
                if event.get("recurrence_interval_days")
            ),
            None,
        )
        try:
            interval_days = int(round(float(interval_value))) if interval_value else 0
        except (TypeError, ValueError):
            interval_days = 0
        dates = sorted(
            event_date
            for event in series
            if (event_date := _recurrence_anchor_date(event)) is not None
        )
        if not dates:
            continue
        conservative_amount = _series_amount(series, state)

        # Preserve explicitly supplied future occurrences first. This matters
        # for scheduled/pending events that are already in the reconstructed state.
        for event in series:
            event_date = _event_projection_date(event)
            override = _event_override_amount(event, state)
            if event_date is not None and start < event_date <= end and override is not None:
                amount = None if override is _event_override_amount.UNCHANGED else float(override)
                _append_event_flow(flows, event, event_date, amount=amount)

        if interval_days <= 0 or conservative_amount is None:
            continue
        cursor = dates[-1]
        while cursor <= start:
            cursor += timedelta(days=interval_days)
        while cursor <= end:
            direction = str(series[0].get("direction") or "")
            signed = conservative_amount if direction == "credit" else -conservative_amount
            if not _explicit_occurrence_matches(
                state,
                series,
                cursor,
                conservative_amount,
                interval_days,
            ):
                flows.append(
                    CashFlow(
                        date=cursor,
                        amount=signed,
                        event_id=None,
                        source="recurring_projection",
                        description=str(series[0].get("description") or "recurring event"),
                        credit_before_debit=(direction == "credit"),
                    )
                )
            cursor += timedelta(days=interval_days)
    return flows


def _one_time_flows(state: Mapping[str, Any], start: date, end: date) -> list[CashFlow]:
    """Return eligible one-time events strictly after the start date."""
    flows: list[CashFlow] = []
    for event in state.get("one_time_events", []) or []:
        if not _cash_flow_allowed(event):
            continue
        event_date = _event_projection_date(event)
        if event_date is None or not start < event_date <= end:
            continue
        _append_event_flow(flows, event, event_date)
    return flows


def _inclusive_percentile(values: Sequence[float], fraction: float) -> float:
    """Return a deterministic linearly interpolated inclusive percentile."""
    if not values:
        raise ValueError("percentile requires at least one value")
    ordered = sorted(float(value) for value in values)
    if len(ordered) == 1:
        return ordered[0]
    position = (len(ordered) - 1) * fraction
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    weight = position - lower
    return ordered[lower] + (ordered[upper] - ordered[lower]) * weight


def _variable_expense_flows(state: Mapping[str, Any], start: date, end: date) -> list[CashFlow]:
    """Project categories using the evidence-conservative variable policy.

    Policy: require at least five dated, known-amount historical occurrences;
    require the shared recurrence cadence check to pass; then use the inclusive
    linear 20th percentile of historical amounts. Flexible spending changes may
    further lower that baseline, but never raise it.
    """
    # VARIABLE-EXPENSE PROJECTION POLICY
    # -----------------------------------
    # * Five or more occurrences: variable categories have more amount noise
    #   than fixed bills, so three observations are not enough evidence to
    #   trust a category-level forecast.
    # * Regular intervals: use the same max-minus-min cadence tolerance as
    #   recurring detection. Many observations at wildly irregular times do
    #   not establish a repeatable spending obligation.
    # * Inclusive linear 20th percentile: once existence and cadence are
    #   supported, retain known-likely spending rather than inventing an
    #   unsupported expense, while choosing a lower historical amount for the
    #   financially safer interpretation when amounts remain noisy. The
    #   percentile uses position (n - 1) * 0.20 with linear interpolation.
    preferences = state.get("preferences") or {}
    protected = set(preferences.get("expense_categories_to_protect") or [])
    permitted_flexible = set(
        preferences.get("expense_categories_user_is_willing_to_reduce") or []
    ) | set(preferences.get("expense_categories_user_is_willing_to_stop") or [])
    projected_categories = protected | {"groceries", "transport"} | permitted_flexible
    groups: defaultdict[str, list[Mapping[str, Any]]] = defaultdict(list)
    for event in state.get("events", []) or []:
        if event.get("direction") != "debit" or event.get("is_recurring"):
            continue
        if not _cash_flow_allowed(event):
            continue
        category = str(event.get("category") or "")
        if category not in projected_categories:
            continue
        event_date = _event_projection_date(event)
        if event_date is None or event_date > start:
            continue
        groups[category].append(event)

    flows: list[CashFlow] = []
    changes = _change_by_event_id(state)
    for category, members in groups.items():
        historical: list[tuple[date, Mapping[str, Any], float]] = []
        for member in members:
            event_date = _event_projection_date(member)
            amount = _event_amount(member)
            if event_date is not None and amount is not None:
                historical.append((event_date, member, amount))
        if len(historical) < 5:
            continue

        dates = sorted(item[0] for item in historical)
        intervals = [(right - left).days for left, right in zip(dates, dates[1:]) if right > left]
        intervals_regular, median_interval = interval_regularity(intervals)
        if not dates or not intervals_regular or median_interval is None:
            continue
        interval_days = max(1, int(round(median_interval)))
        amounts = [item[2] for item in historical]
        projected_amount = _inclusive_percentile(amounts, 0.20)
        if projected_amount <= 0:
            continue

        member_changes = [changes.get(str(member.get("event_id"))) for _, member, _ in historical]
        if any(change and change.get("action") == "stop" for change in member_changes):
            continue
        reductions = [
            float(change["new_amount"])
            for change in member_changes
            if change and change.get("action") == "reduce_to"
        ]
        if reductions:
            projected_amount = min(projected_amount, min(reductions))
        if projected_amount <= 0:
            continue

        cursor = dates[-1]
        while cursor <= start:
            cursor += timedelta(days=interval_days)
        while cursor <= end:
            flows.append(
                CashFlow(
                    date=cursor,
                    amount=-projected_amount,
                    event_id=None,
                    source="variable_projection",
                    description=f"essential variable spending: {category}",
                )
            )
            cursor += timedelta(days=interval_days)
    return flows


def _flow_sort_key(flow: CashFlow) -> tuple[Any, ...]:
    """Order confirmed income before same-day debits, uncertain credits after."""
    if flow.amount > 0 and flow.credit_before_debit:
        same_day_order = 0
    elif flow.amount < 0:
        same_day_order = 1
    else:
        same_day_order = 2
    return (flow.date, same_day_order, flow.event_id or "", flow.description)


def _normalize_payments(
    payments: Mapping[date | str, float] | Iterable[Mapping[str, Any] | Sequence[Any]] | None,
) -> list[tuple[date, float]]:
    """Normalize plan payments to dated non-negative debit amounts."""
    if payments is None:
        return []
    normalized: list[tuple[date, float]] = []
    if isinstance(payments, Mapping):
        items = payments.items()
    else:
        items = payments
    for item in items:
        if isinstance(payments, Mapping):
            payment_date, amount = item
        elif isinstance(item, Mapping):
            payment_date = item.get("date") or item.get("payment_date")
            amount = item.get("amount")
        else:
            payment_date, amount = item[0], item[1]
        if payment_date is None:
            continue
        numeric = float(amount)
        if not math.isfinite(numeric) or numeric < 0:
            raise ValueError(f"Invalid scheduled payment amount: {amount!r}")
        normalized.append((_as_date(payment_date), numeric))
    return normalized


def build_projected_cash_flows(
    state: Mapping[str, Any], start_date: date | str, horizon_days: int = 90
) -> list[CashFlow]:
    """Build deterministic event and recurring flows after ``start_date``."""
    start = _as_date(start_date)
    if horizon_days < 0:
        raise ValueError("horizon_days must be non-negative")
    end = start + timedelta(days=horizon_days)
    # Boundary convention: projected event flows use (start, end], while the
    # simulator separately includes day-0 scheduled payments and day 0..90.
    flows = (
        _recurring_flows(state, start, end)
        + _one_time_flows(state, start, end)
        + _variable_expense_flows(state, start, end)
    )
    return sorted(flows, key=_flow_sort_key)


def simulate_90_days(
    state: Mapping[str, Any],
    start_date: date | str,
    scheduled_payments: (
        Mapping[date | str, float]
        | Iterable[Mapping[str, Any] | Sequence[Any]]
        | None
    ) = None,
    *,
    horizon_days: int = 90,
) -> dict[str, Any]:
    """Simulate balances from ``start_date`` through the inclusive 90-day horizon.

    Debits are applied before credits on the same date, which is the conservative
    interpretation when same-day ordering is not supplied. Every payment and
    projected event is visible in ``daily_balances`` and ``cash_flows``.
    """
    start = _as_date(start_date)
    end = start + timedelta(days=horizon_days)
    # Boundary convention: scheduled payments use [start, end] and the daily
    # simulation visits every day from start through end inclusively.
    minimum = float(state.get("minimum_balance_to_keep") or 0.0)
    balance = float(state.get("current_balance") or 0.0)
    flows = build_projected_cash_flows(state, start, horizon_days)
    for payment_date, amount in _normalize_payments(scheduled_payments):
        if start <= payment_date <= end and amount:
            flows.append(
                CashFlow(
                    date=payment_date,
                    amount=-amount,
                    event_id=None,
                    source="scheduled_payment",
                    description="recommended payment",
                )
            )
    flows.sort(key=_flow_sort_key)

    by_date: defaultdict[date, list[CashFlow]] = defaultdict(list)
    for flow in flows:
        by_date[flow.date].append(flow)

    daily: list[dict[str, Any]] = []
    applied: list[CashFlow] = []
    minimum_seen = balance
    first_breach: date | None = start if balance < minimum else None
    for offset in range(horizon_days + 1):
        current = start + timedelta(days=offset)
        opening = balance
        day_flows: list[dict[str, Any]] = []
        for flow in by_date.get(current, []):
            balance += flow.amount
            applied.append(flow)
            minimum_seen = min(minimum_seen, balance)
            if first_breach is None and balance < minimum:
                first_breach = current
            day_flows.append(flow.as_dict() | {"balance_after": balance})
        daily.append(
            {
                "date": current.isoformat(),
                "opening_balance": opening,
                "flows": day_flows,
                "closing_balance": balance,
                "safe": first_breach is None or current < first_breach,
            }
        )

    return {
        "start_date": start.isoformat(),
        "end_date": end.isoformat(),
        "horizon_days": horizon_days,
        "minimum_balance_to_keep": minimum,
        "starting_balance": float(state.get("current_balance") or 0.0),
        "ending_balance": balance,
        "minimum_projected_balance": minimum_seen,
        "safe": first_breach is None,
        "first_breach_date": first_breach.isoformat() if first_breach else None,
        "cash_flows": [flow.as_dict() for flow in applied],
        "daily_balances": daily,
    }


def amount_safe_to_pay_today(
    state: Mapping[str, Any],
    start_date: date | str,
    requested_amount: float,
    *,
    horizon_days: int = 90,
) -> float:
    """Return the largest immediate payment that preserves the minimum balance."""
    requested = max(0.0, float(requested_amount))
    if requested == 0:
        return 0.0

    def safe(amount: float) -> bool:
        """Check whether an immediate payment preserves the minimum balance."""
        result = simulate_90_days(
            state,
            start_date,
            scheduled_payments=[(_as_date(start_date), amount)],
            horizon_days=horizon_days,
        )
        return bool(result["safe"])

    if not safe(0.0):
        return 0.0
    low, high = 0.0, requested
    for _ in range(60):
        middle = (low + high) / 2
        if safe(middle):
            low = middle
        else:
            high = middle
    return min(requested, max(0.0, low))


def earliest_safe_full_payment_date(
    state: Mapping[str, Any],
    start_date: date | str,
    requested_amount: float,
    *,
    horizon_days: int = 90,
) -> date | None:
    """Find the first date a single full payment remains safe through the horizon."""
    start = _as_date(start_date)
    amount = max(0.0, float(requested_amount))
    for offset in range(horizon_days + 1):
        candidate = start + timedelta(days=offset)
        result = simulate_90_days(
            state,
            start,
            scheduled_payments=[(candidate, amount)],
            horizon_days=horizon_days,
        )
        if result["safe"]:
            return candidate
    return None


# Descriptive aliases for later plan-selector code.
simulate_balance = simulate_90_days
find_earliest_safe_full_payment = earliest_safe_full_payment_date

