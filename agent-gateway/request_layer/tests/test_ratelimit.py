"""Unit tests for the L5 rate gates.

The token-bucket Lua script is exercised against a real Redis in
integration testing; here we test the Python helpers (cost estimator,
rate-table fallback, response-header parsing).
"""

import pytest

from request_layer.src.ratelimit import estimate_cost


def test_estimate_cost_uses_response_headers_when_present() -> None:
    cost = estimate_cost(
        model="gpt-4o-mini",
        body_in="hello",
        body_out="bonjour",
        headers={"X-Input-Tokens": "100", "X-Output-Tokens": "50"},
    )
    # gpt-4o-mini = (0.00000015, 0.0000006); 100*1.5e-7 + 50*6e-7 = 4.5e-5
    assert cost == _approx(0.0000150 + 0.0000300)


def test_estimate_cost_falls_back_to_char_estimate() -> None:
    cost = estimate_cost(
        model="gpt-4o-mini",
        body_in="hello world" * 4,  # 44 chars → ~11 tokens
        body_out="bonjour le monde",  # 16 chars → 4 tokens
        headers={},
    )
    assert cost > 0


def test_estimate_cost_unknown_model_uses_default_rate() -> None:
    cost = estimate_cost(
        model="some-novel-model",
        body_in="x" * 4,  # 1 token
        body_out="y" * 4,  # 1 token
        headers={},
    )
    # Default rate (0.000001 in, 0.000003 out) → 4e-6
    assert cost == _approx(0.000001 + 0.000003)


def test_estimate_cost_handles_none_model() -> None:
    cost = estimate_cost(
        model=None,
        body_in="hi",
        body_out="hi",
        headers={},
    )
    assert cost > 0


def _approx(value: float, rel: float = 1e-3) -> object:
    """Local wrapper around ``pytest.approx`` for terser test assertions."""

    return pytest.approx(value, rel=rel)
