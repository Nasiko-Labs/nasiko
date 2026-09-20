"""Select deterministic safe payment plans and apply the challenge tie-break rules.

This module has no LLM dependency. It computes each method's eligibility from
user preferences and request fields, tests every candidate with
``forecast_engine``, optionally searches up to three permitted flexible-expense
changes, and returns one validated decision with a chronological payment plan.
"""

from __future__ import annotations

import copy
import itertools
from dataclasses import dataclass
from decimal import Decimal, ROUND_HALF_UP
from datetime import date, timedelta
from typing import Any, Iterable, Mapping, Sequence

from forecast_engine import (
    amount_safe_to_pay_today,
    earliest_safe_full_payment_date,
    simulate_90_days,
)

_ALLOWED_METHODS = frozenset(
    {"full_payment", "partial_payment", "installments", "wait", "not_recommended"}
)


def _as_date(value: date | str) -> date:
    """Normalize an ISO date string or date object."""
    return value if isinstance(value, date) else date.fromisoformat(str(value)[:10])


def _as_bool(value: Any) -> bool:
    """Interpret the dataset's common boolean representations."""
    return value is True or str(value).strip().lower() in {"true", "1", "yes"}


def _number(value: Any, default: float = 0.0) -> float:
    """Parse a numeric field with a deterministic fallback."""
    try:
        return float(value) if value is not None else default
    except (TypeError, ValueError):
        return default


def _money(value: float) -> Decimal:
    """Round a monetary value to cents using financial half-up rounding."""
    return Decimal(str(value)).quantize(Decimal("0.01"), rounding=ROUND_HALF_UP)


def _format_amount(value: float) -> str:
    """Format a rounded amount without unnecessary trailing zeroes."""
    formatted = f"{float(_money(value)):.2f}".rstrip("0").rstrip(".")
    return formatted if formatted else "0"


def _format_plan(payments: Sequence[tuple[date, float]]) -> str:
    """Serialize payments chronologically in the output contract format."""
    ordered = sorted(payments, key=lambda item: item[0])
    return (
        "|".join(f"{day.isoformat()}:{_format_amount(amount)}" for day, amount in ordered)
        or "none"
    )


def _record_run_item(run_summary: dict[str, Any] | None, key: str, item: Mapping[str, Any]) -> None:
    """Append optional plan-validation telemetry to the run summary."""
    if run_summary is not None:
        run_summary.setdefault(key, []).append(dict(item))


def _check_payment_sum(
    run_summary: dict[str, Any] | None,
    request_id: Any,
    payment_type: str,
    amounts: Sequence[float],
    requested: float,
    option: Mapping[str, Any] | None = None,
) -> bool:
    """Check a plan total against its contractual expected amount."""
    total = sum((_money(amount) for amount in amounts), Decimal("0"))
    if payment_type == "installments" and option is not None:
        total_payable = option.get("total_payable_amount")
        expected = _money(total_payable if total_payable is not None else requested)
        financing_fee = option.get("financing_fee")
        if total_payable is not None and financing_fee is not None:
            principal_plus_fee = _money(requested) + _money(financing_fee)
            fee_mismatch = expected - principal_plus_fee
            _record_run_item(
                run_summary,
                "installment_fee_consistency_checks",
                {
                    "request_id": request_id,
                    "payment_option_id": option.get("payment_option_id"),
                    "total_payable_amount": float(expected),
                    "requested_amount_plus_financing_fee": float(principal_plus_fee),
                    "mismatch_amount": float(fee_mismatch),
                    "within_tolerance": abs(fee_mismatch) <= Decimal("0.01"),
                },
            )
    else:
        expected = _money(requested)
    mismatch = total - expected
    if abs(mismatch) > Decimal("0.01"):
        _record_run_item(
            run_summary,
            "payment_sum_assertion_failures",
            {
                "request_id": request_id,
                "payment_type": payment_type,
                "expected": float(expected),
                "actual": float(total),
                "mismatch_amount": float(mismatch),
            },
        )
        return False
    return True


def _methods_user_accepts(state: Mapping[str, Any]) -> set[str]:
    """Return payment methods permitted by the user's preferences."""
    preferences = state.get("preferences") or {}
    methods = preferences.get("payment_methods_user_will_consider")
    if methods is None:
        methods = state.get("payment_preferences") or []
    return {str(method) for method in methods}


def _option_months_allowed(option: Mapping[str, Any], max_months: Any) -> bool:
    """Check an installment option against the user's duration limit."""
    if max_months in (None, ""):
        return True
    first = option.get("first_payment_date")
    count = int(option.get("number_of_payments") or 0)
    frequency = int(option.get("payment_frequency_days") or 0)
    if not first or count <= 0 or frequency <= 0:
        return False
    try:
        duration_days = (count - 1) * frequency
        return duration_days <= float(max_months) * 31.0
    except (TypeError, ValueError):
        return False


def _installment_payments(option: Mapping[str, Any]) -> list[tuple[date, float]] | None:
    """Expand a supplied installment option into dated payments."""
    first = option.get("first_payment_date")
    count = int(option.get("number_of_payments") or 0)
    frequency = int(option.get("payment_frequency_days") or 0)
    amount = float(_money(_number(option.get("payment_amount"))))
    if not first or count <= 0 or frequency <= 0 or amount <= 0:
        return None
    first_date = _as_date(first)
    return [(first_date + timedelta(days=index * frequency), amount) for index in range(count)]


def _candidate_actions(state: Mapping[str, Any]) -> list[dict[str, Any]]:
    """Return permitted, deterministic stop/reduce actions for recurring expenses."""
    preferences = state.get("preferences") or {}
    can_stop = set(preferences.get("expense_categories_user_is_willing_to_stop") or [])
    can_reduce = set(preferences.get("expense_categories_user_is_willing_to_reduce") or [])
    # Prefer the newest normalized event for each description/category series,
    # but scan all events regardless of whether reconstruction bucketed the
    # series as recurring or one-time.
    source_events = list(state.get("events", []) or [])
    if not source_events:
        source_events = list(state.get("confirmed_recurring_expenses", []) or [])
    latest_by_series: dict[tuple[str, ...], Mapping[str, Any]] = {}
    for event in source_events:
        if event.get("direction") != "debit":
            continue
        series = (
            str(event.get("event_type", "")),
            str(event.get("direction", "")),
            str(event.get("category", "")),
            str(event.get("description", "")),
            str(event.get("currency", "")),
        )
        current = latest_by_series.get(series)
        current_date = str(current.get("event_date") or "") if current else ""
        event_date = str(event.get("event_date") or "")
        if current is None or event_date >= current_date:
            latest_by_series[series] = event
    actions: list[dict[str, Any]] = []
    for event in latest_by_series.values():
        event_id = event.get("event_id")
        category = str(event.get("category", ""))
        flexibility = str(event.get("flexibility") or "")
        current = _number(event.get("amount_home_currency", event.get("amount")))
        if not event_id or current <= 0 or event.get("direction") != "debit":
            continue
        if category in can_stop and flexibility in {"stoppable", "reducible_or_stoppable"}:
            actions.append({"event_id": str(event_id), "action": "stop", "new_amount": 0.0})
        minimum = event.get(
            "minimum_allowed_amount_home_currency",
            event.get("minimum_allowed_amount"),
        )
        if (
            category in can_reduce
            and flexibility in {"reducible", "reducible_or_stoppable"}
            and minimum is not None
        ):
            new_amount = _number(minimum)
            # minimum_allowed_amount is treated as an inclusive floor — a reduction may reach exactly this value, since the problem statement provides no "strictly above" requirement.
            if 0 <= new_amount < current:
                actions.append(
                    {"event_id": str(event_id), "action": "reduce_to", "new_amount": new_amount}
                )
    # Try the largest reductions first; event ID keeps equal savings deterministic.
    actions.sort(
        key=lambda action: (
            -float(action["new_amount"] == 0) * 10
            - float(action.get("new_amount", 0)),
            str(action["event_id"]),
        )
    )
    return actions


def _state_with_changes(
    state: Mapping[str, Any], changes: Sequence[Mapping[str, Any]]
) -> dict[str, Any]:
    """Copy a state and attach the candidate flexible-spending changes."""
    updated = copy.deepcopy(dict(state))
    updated["spending_changes"] = [dict(change) for change in changes]
    return updated


def _find_safe_changes(
    state: Mapping[str, Any],
    request_date: date,
    payments: Sequence[tuple[date, float]],
    actions: Sequence[Mapping[str, Any]],
    horizon_days: int,
) -> list[dict[str, Any]] | None:
    """Find the fewest permitted changes that make a payment plan safe."""
    def safe(changes: Sequence[Mapping[str, Any]]) -> bool:
        """Check whether the plan remains safe after these changes."""
        return bool(
            simulate_90_days(
                _state_with_changes(state, changes),
                request_date,
                scheduled_payments=payments,
                horizon_days=horizon_days,
            )["safe"]
        )

    if safe([]):
        return []
    # Bound the search while still covering the strongest deterministic actions.
    candidates = list(actions[:14])
    for size in range(1, min(3, len(candidates)) + 1):
        for combination in itertools.combinations(candidates, size):
            event_ids = [str(change["event_id"]) for change in combination]
            if len(event_ids) != len(set(event_ids)):
                continue
            changes = [dict(change) for change in combination]
            if safe(changes):
                return changes
    return None


@dataclass(frozen=True)
class _Candidate:
    method: str
    status: str
    payments: tuple[tuple[date, float], ...]
    total_paid: float
    changes: tuple[dict[str, Any], ...]
    option_id: str
    completes_by_deadline: bool

    @property
    def start_date(self) -> date:
        """Return the first date on which this candidate makes a payment."""
        return min(payment[0] for payment in self.payments)

    def tie_key(self) -> tuple[Any, ...]:
        """Exact six-level preference ordering; lower tuple values win."""
        return (
            0 if self.completes_by_deadline else 1,
            0 if not self.changes else 1,
            round(self.total_paid, 8),
            self.start_date,
            len(self.payments),
            self.option_id,
        )


def _build_candidate(
    *,
    method: str,
    status: str,
    payments: Sequence[tuple[date, float]],
    option_id: str,
    state: Mapping[str, Any],
    request_date: date,
    deadline: date,
    actions: Sequence[Mapping[str, Any]],
    horizon_days: int,
) -> _Candidate | None:
    """Build a safe candidate, or return ``None`` when it is infeasible."""
    ordered = tuple(sorted(payments, key=lambda item: item[0]))
    if (
        not ordered
        or ordered[-1][0] > deadline
        or ordered[-1][0] > request_date + timedelta(days=horizon_days)
    ):
        return None
    changes = _find_safe_changes(state, request_date, ordered, actions, horizon_days)
    if changes is None:
        return None
    return _Candidate(
        method=method,
        status=status,
        payments=ordered,
        total_paid=sum(amount for _, amount in ordered),
        changes=tuple(dict(change) for change in changes),
        option_id=option_id,
        completes_by_deadline=ordered[-1][0] <= deadline,
    )


def select_plan(
    request: Mapping[str, Any],
    state: Mapping[str, Any],
    payment_options: Iterable[Mapping[str, Any]] = (),
    *,
    horizon_days: int = 90,
    run_summary: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Select and return the safest eligible decision for one request."""
    request_date = _as_date(request["request_date"])
    deadline = _as_date(request["desired_completion_date"])
    requested = max(0.0, _number(request.get("requested_amount")))
    accepted = _methods_user_accepts(state)
    amount_safe = float(
        _money(
            min(
                requested,
                max(
                    0.0,
                    amount_safe_to_pay_today(
                        state, request_date, requested, horizon_days=horizon_days
                    ),
                ),
            )
        )
    )
    earliest = earliest_safe_full_payment_date(
        state, request_date, requested, horizon_days=horizon_days
    )
    actions = _candidate_actions(state)
    candidates: list[_Candidate] = []

    if requested == 0:
        candidates.append(
            _Candidate(
                "full_payment",
                "affordable_now",
                ((request_date, 0.0),),
                0.0,
                tuple(),
                "",
                True,
            )
        )
    if "full_payment" in accepted:
        # Try today even when the no-change earliest date is later: an eligible
        # stop/reduce action may make an otherwise unsafe immediate payment safe.
        candidate = _build_candidate(
            method="full_payment",
            status="affordable_now",
            payments=[(request_date, requested)],
            option_id="",
            state=state,
            request_date=request_date,
            deadline=deadline,
            actions=actions,
            horizon_days=horizon_days,
        )
        if candidate:
            candidates.append(candidate)
    if "full_payment" in accepted and earliest is not None and earliest > request_date:
        candidate = _build_candidate(
            method="wait",
            status="affordable_later",
            payments=[(earliest, requested)],
            option_id="",
            state=state,
            request_date=request_date,
            deadline=deadline,
            actions=actions,
            horizon_days=horizon_days,
        )
        if candidate:
            candidates.append(candidate)
    if (
        "partial_payment" in accepted
        and _as_bool(request.get("allows_partial_payment"))
        and 0 < amount_safe < requested
        and earliest is not None
        and request_date < earliest <= deadline
    ):
        requested_money = _money(requested)
        partial_today_money = min(_money(amount_safe), requested_money)
        partial_remaining_money = _money(requested_money - partial_today_money)
        partial_today = float(partial_today_money)
        partial_remaining = float(partial_remaining_money)
        _check_payment_sum(
            run_summary,
            request.get("request_id"),
            "partial_payment",
            [partial_today, partial_remaining],
            requested,
        )
        candidate = None
        if partial_today > 0 and partial_remaining > 0:
            candidate = _build_candidate(
                method="partial_payment",
                status="affordable_with_plan",
                payments=[(request_date, partial_today), (earliest, partial_remaining)],
                option_id="",
                state=state,
                request_date=request_date,
                deadline=deadline,
                actions=actions,
                horizon_days=horizon_days,
            )
        if candidate and _check_payment_sum(
            run_summary,
            request.get("request_id"),
            "partial_payment_candidate",
            [amount for _, amount in candidate.payments],
            requested,
        ):
            candidates.append(candidate)

    if "installments" in accepted:
        max_months = (state.get("preferences") or {}).get("max_installment_months")
        for option in sorted(
            payment_options, key=lambda row: str(row.get("payment_option_id", ""))
        ):
            if (
                option.get("payment_method") != "installments"
                or not _option_months_allowed(option, max_months)
            ):
                continue
            payments = _installment_payments(option)
            if not payments:
                continue
            _check_payment_sum(
                run_summary,
                request.get("request_id"),
                "installments",
                [amount for _, amount in payments],
                requested,
                option=option,
            )
            option_id = str(option.get("payment_option_id", ""))
            candidate = _build_candidate(
                method="installments",
                status="affordable_with_plan",
                payments=payments,
                option_id=option_id,
                state=state,
                request_date=request_date,
                deadline=deadline,
                actions=actions,
                horizon_days=horizon_days,
            )
            if candidate:
                candidates.append(candidate)

    candidates.sort(key=lambda candidate: candidate.tie_key())
    chosen = candidates[0] if candidates else None
    if chosen is None:
        status = "affordable_later" if earliest is not None else "not_affordable"
        return {
            "request_id": request.get("request_id"),
            "amount_safe_to_pay": min(requested, max(0.0, amount_safe)),
            "affordability_status": status,
            "recommended_payment_method": "not_recommended",
            "payment_plan": "none",
            "earliest_date_for_full_payment": earliest.isoformat() if earliest else "",
            "spending_changes_needed": "none",
            "decision_explanation": (
                "No safe eligible payment plan completes by the requested deadline."
            ),
            "eligible_methods": sorted(accepted & _ALLOWED_METHODS),
            "candidate_count": 0,
        }

    if not 0 <= amount_safe <= requested:
        raise ValueError("safe payment amount is outside the requested amount bounds")
    changes = "|".join(
        f"stop:{change['event_id']}"
        if change["action"] == "stop"
        else f"reduce_to:{change['event_id']}:{_format_amount(float(change['new_amount']))}"
        for change in chosen.changes
    ) or "none"
    # ``earliest`` describes the no-change forecast used for wait/partial
    # eligibility. Once a same-day candidate is made safe by permitted changes,
    # the output contract requires its reported earliest date to be today.
    reported_earliest = request_date if chosen.status == "affordable_now" else earliest
    return {
        "request_id": request.get("request_id"),
        "amount_safe_to_pay": amount_safe,
        "affordability_status": chosen.status,
        "recommended_payment_method": chosen.method,
        "payment_plan": _format_plan(chosen.payments),
        "earliest_date_for_full_payment": (
            reported_earliest.isoformat() if reported_earliest else ""
        ),
        "spending_changes_needed": changes,
        "decision_explanation": f"Use {chosen.method} while keeping the minimum balance protected.",
        "eligible_methods": sorted(accepted & _ALLOWED_METHODS),
        "candidate_count": len(candidates),
        "chosen_payment_option_id": chosen.option_id or None,
    }


select_payment_plan = select_plan
