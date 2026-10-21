import json
from unittest.mock import patch
import httpx
import pytest
from httpx import ASGITransport, AsyncClient

from server.main import app
from server.schemas import (
    CompanyContext,
    CompetitiveSignal,
    DepartmentImpact,
    SignalOverlap,
)
from server import dronahq


@pytest.fixture
def sample_rich_signal():
    return CompetitiveSignal(
        competitor="Cashfree",
        category="product",
        headline="Cashfree launches RiskShield for real-time payment fraud prevention",
        summary="RiskShield offers automated transaction blocking and fraud risk scoring for ecommerce merchants.",
        significance="SIGNIFICANT",
        overlap=SignalOverlap(
            products=["FraudShield", "Payment Gateway"],
            customer_segments=["SMB", "Mid-Market"],
        ),
        impact=DepartmentImpact(
            product="HIGH",
            sales="MEDIUM",
            marketing="MEDIUM",
            strategy="LOW",
        ),
        recommended_actions=[
            "Investigate whether FraudShield needs real-time velocity checking.",
            "Sales talk track: Highlight PayFlow's native gateway integration and zero chargeback guarantee.",
        ],
        confidence="high",
        source_urls=["https://cashfree.com/riskshield-announcement"],
    )


@pytest.mark.asyncio
async def test_simulate_endpoint_valid_rich_signal(sample_rich_signal):
    """1. Test valid rich CompetitiveSignal simulation via POST /api/simulate with mocked DronaHQ."""
    mock_dronahq_response = {
        "success": True,
        "thread_id": "sim_thread_1",
        "run_id": "sim_run_1",
        "message": "Simulation options generated.",
        "response": {
            "product": {
                "actions": ["Evaluate FraudShield rule latency against RiskShield benchmarks."],
                "investigation_questions": ["What is Cashfree's claimed false positive rate?"],
                "caveats": ["Primary benchmark data required before concluding performance delta."],
            },
            "sales": {
                "trigger": "Prospect mentions Cashfree RiskShield during evaluation.",
                "talk_track": "Focus on PayFlow's end-to-end reconciliation and chargeback protection.",
                "questions": ["Are you experiencing fraud in card payments or UPI specifically?"],
                "caveats": ["Do not claim features without technical verification."],
            },
            "marketing": {
                "actions": ["Update FraudShield product page with real-time detection highlights."],
                "messaging_angles": ["Zero-friction merchant security."],
                "caveats": ["Avoid unsupported superiority claims."],
            },
            "strategy": {
                "questions": ["Is RiskShield gaining traction in mid-market merchants?"],
                "actions": ["Track competitive win/loss metrics."],
                "caveats": ["Decision support only."],
            },
            "confidence": "high",
        },
    }

    def handler(request: httpx.Request):
        assert request.url == "https://mock.dronahq.com/webhook/test"
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as ac:
                    resp = await ac.post(
                        "/api/simulate",
                        json={"signal": sample_rich_signal.model_dump()},
                    )

    assert resp.status_code == 200
    data = resp.json()

    # Verify context
    assert data["signal_context"]["competitor"] == "Cashfree"
    assert data["signal_context"]["headline"] == sample_rich_signal.headline
    assert data["signal_context"]["significance"] == "SIGNIFICANT"

    # Verify sections
    assert "Evaluate FraudShield rule latency" in data["product"]["actions"][0]
    assert data["sales"]["trigger"] == "Prospect mentions Cashfree RiskShield during evaluation."
    assert "Zero-friction merchant security." in data["marketing"]["messaging_angles"]
    assert "Is RiskShield gaining traction" in data["strategy"]["questions"][0]

    # Verify evidence & confidence
    assert data["evidence"]["source_urls"] == ["https://cashfree.com/riskshield-announcement"]
    assert data["confidence"] == "high"


@pytest.mark.asyncio
async def test_simulate_dronahq_json_string_response(sample_rich_signal):
    """2 & 3. Test DronaHQ response where 'response' field is a stringified JSON."""
    structured = {
        "product": {
            "actions": ["Investigate rules engine."],
            "investigation_questions": ["What rules does Cashfree support?"],
            "caveats": ["Validate via docs."],
        },
        "sales": {
            "trigger": "Prospect objection on fraud.",
            "talk_track": "PayFlow reliability.",
            "questions": ["Which payment modes?"],
            "caveats": ["No unverified claims."],
        },
        "marketing": {
            "actions": ["Create battlecard."],
            "messaging_angles": ["Better accuracy."],
            "caveats": ["Validate metrics."],
        },
        "strategy": {
            "questions": ["Trend or one-off?"],
            "actions": ["Monitor pipeline."],
            "caveats": ["Guidance only."],
        },
        "confidence": "high",
    }
    mock_dronahq_response = {
        "success": True,
        "thread_id": "sim_thread_2",
        "run_id": "sim_run_2",
        "message": "Success",
        "response": json.dumps(structured),
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                res = await dronahq.simulate_response(sample_rich_signal)

    assert res.product.actions == ["Investigate rules engine."]
    assert res.sales.talk_track == "PayFlow reliability."
    assert res.marketing.messaging_angles == ["Better accuracy."]
    assert res.strategy.questions == ["Trend or one-off?"]
    assert res.evidence.source_urls == ["https://cashfree.com/riskshield-announcement"]


@pytest.mark.asyncio
async def test_simulate_dronahq_markdown_fenced_response(sample_rich_signal):
    """4. Test DronaHQ response where 'response' is enclosed in markdown code fences."""
    fenced = (
        "```json\n"
        "{\n"
        '  "product": {"actions": ["Audit FraudShield"], "investigation_questions": ["Is it rule-based?"], "caveats": ["Check docs"]},\n'
        '  "sales": {"trigger": "Competitor mention", "talk_track": "Our strengths", "questions": ["Timeline?"], "caveats": ["Honest selling"]},\n'
        '  "marketing": {"actions": ["One-pager"], "messaging_angles": ["Security first"], "caveats": ["No unsubstantiated claims"]},\n'
        '  "strategy": {"questions": ["Market direction?"], "actions": ["Interview lost deals"], "caveats": ["Support only"]},\n'
        '  "confidence": "medium"\n'
        "}\n"
        "```"
    )
    mock_dronahq_response = {
        "success": True,
        "thread_id": "sim_thread_3",
        "run_id": "sim_run_3",
        "message": "Done",
        "response": fenced,
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                res = await dronahq.simulate_response(sample_rich_signal)

    assert res.product.actions == ["Audit FraudShield"]
    assert res.sales.trigger == "Competitor mention"
    assert res.marketing.actions == ["One-pager"]
    assert res.strategy.actions == ["Interview lost deals"]
    assert res.confidence == "medium"
    assert res.evidence.source_urls == sample_rich_signal.source_urls


@pytest.mark.asyncio
async def test_simulate_dronahq_malformed_response_fallback(sample_rich_signal):
    """5. Test DronaHQ returning an unparseable response string gracefully triggering fallback."""
    mock_dronahq_response = {
        "success": True,
        "thread_id": "sim_thread_4",
        "run_id": "sim_run_4",
        "message": "Success",
        "response": "Here are some general thoughts about competition without proper JSON formatting...",
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                res = await dronahq.simulate_response(sample_rich_signal)

    # Must not crash; returns fallback simulation
    assert res.signal_context.competitor == "Cashfree"
    assert len(res.product.actions) > 0
    assert len(res.product.investigation_questions) > 0
    assert len(res.sales.questions) > 0
    assert len(res.marketing.messaging_angles) > 0
    assert len(res.strategy.questions) > 0
    # URLs preserved
    assert res.evidence.source_urls == sample_rich_signal.source_urls
    # Fallback caveats present
    assert any("AI simulation fallback" in c or "Competitor capabilities" in c for c in res.product.caveats)


@pytest.mark.asyncio
async def test_simulate_dronahq_http_failure_fallback(sample_rich_signal):
    """6. Test DronaHQ HTTP 500 error gracefully triggering fallback without crashing."""
    def handler(request: httpx.Request):
        return httpx.Response(500, text="Internal Server Error")

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                res = await dronahq.simulate_response(sample_rich_signal)

    assert res.signal_context.competitor == "Cashfree"
    assert res.evidence.source_urls == sample_rich_signal.source_urls
    assert any("500" in c or "AI simulation fallback" in c for c in res.product.caveats)


@pytest.mark.asyncio
async def test_simulate_signal_missing_optional_fields():
    """7. Test minimal CompetitiveSignal with missing optional fields."""
    minimal_signal = CompetitiveSignal(
        competitor="PayU",
        category=None,
        headline=None,
        summary=None,
        significance=None,
        overlap=SignalOverlap(),
        impact=DepartmentImpact(),
        recommended_actions=[],
        confidence=None,
        source_urls=["https://payu.in/news"],
    )

    def handler(request: httpx.Request):
        return httpx.Response(500, text="Service Unavailable")

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                res = await dronahq.simulate_response(minimal_signal)

    assert res.signal_context.competitor == "PayU"
    assert res.signal_context.headline is None
    assert res.evidence.source_urls == ["https://payu.in/news"]
    assert len(res.product.actions) > 0
    assert len(res.sales.questions) > 0
    assert len(res.marketing.actions) > 0
    assert len(res.strategy.questions) > 0


@pytest.mark.asyncio
async def test_simulate_signal_with_no_source_urls():
    """8 & 10. Test signal with empty source_urls enforces low confidence & caveats without fabricating sources."""
    no_source_signal = CompetitiveSignal(
        competitor="Razorpay",
        category="product",
        headline="Razorpay updates fee structure",
        summary="Unverified rumors regarding fee adjustment.",
        significance="INFORMATIONAL",
        overlap=SignalOverlap(products=["Payment Gateway"], customer_segments=["SMB"]),
        impact=DepartmentImpact(),
        recommended_actions=[],
        confidence="low",
        source_urls=[],
    )

    mock_dronahq_response = {
        "success": True,
        "response": {
            "product": {"actions": ["Investigate pricing"]},
            "sales": {"trigger": "Prospect asks about fee cut"},
            "marketing": {"actions": ["Maintain fee transparency"]},
            "strategy": {"questions": ["Is margin compression expected?"]},
            "confidence": "high",  # Even if agent said high, empty sources should force low confidence
        },
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                res = await dronahq.simulate_response(no_source_signal)

    # Empty source URLs must be preserved (not fabricated)
    assert res.evidence.source_urls == []
    # Confidence must be low
    assert res.confidence == "low"
    # Caveats must explicitly flag insufficient evidence
    assert any("insufficient" in c.lower() for c in res.product.caveats)
    assert any("insufficient" in c.lower() for c in res.sales.caveats)
    assert any("insufficient" in c.lower() for c in res.marketing.caveats)
    assert any("insufficient" in c.lower() for c in res.strategy.caveats)


@pytest.mark.asyncio
async def test_simulate_preserves_existing_recommended_actions(sample_rich_signal):
    """11. Test that existing rich signal recommended_actions and sales talk track are reused."""
    def handler(request: httpx.Request):
        return httpx.Response(500, text="Offline")

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                res = await dronahq.simulate_response(sample_rich_signal)

    # Product action should incorporate the non-sales recommendation
    assert any("real-time velocity checking" in a for a in res.product.actions)
    # Sales talk track should incorporate the existing talk track from recommended_actions
    assert "native gateway integration and zero chargeback guarantee" in res.sales.talk_track
