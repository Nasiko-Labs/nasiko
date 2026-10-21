import json
from unittest.mock import patch
import httpx
import pytest
from httpx import ASGITransport, AsyncClient

from server.main import app
from server.schemas import (
    CompetitiveSignal,
    DepartmentImpact,
    SignalOverlap,
)
from server import dronahq


@pytest.fixture
def sample_signals():
    return [
        CompetitiveSignal(
            competitor="Cashfree",
            category="product",
            headline="Launch of 'RiskShield', a real-time risk management solution for cyber payment frauds.",
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
                "Investigate machine learning capabilities for FraudShield.",
                "Sales talk track: Highlight PayFlow's chargeback guarantee.",
            ],
            confidence="high",
            source_urls=[
                "https://cashfree.com/riskshield-announcement",
                "https://cashfree.com/risk-shield-overview",
            ],
        ),
        CompetitiveSignal(
            competitor="Razorpay",
            category="product",
            headline="Introduction of Chargeback Shield with real-time transaction monitoring.",
            summary="Razorpay provides chargeback defense for digital payments.",
            significance="INFORMATIONAL",
            overlap=SignalOverlap(
                products=["FraudShield"],
                customer_segments=["Mid-Market"],
            ),
            impact=DepartmentImpact(
                product="MEDIUM",
                sales="LOW",
                marketing="MEDIUM",
                strategy="LOW",
            ),
            recommended_actions=[
                "Monitor merchant adoption of Chargeback Shield.",
            ],
            confidence="medium",
            source_urls=[
                "https://razorpay.com/chargeback-shield",
            ],
        ),
    ]


@pytest.mark.asyncio
async def test_ask_endpoint_valid_structured_json(sample_signals):
    """1 & 2. Test valid question with rich signals and structured DronaHQ JSON response."""
    mock_dronahq_response = {
        "success": True,
        "thread_id": "ask_thread_1",
        "run_id": "ask_run_1",
        "message": "Reasoning complete.",
        "response": {
            "answer": "Cashfree's RiskShield directly competes with PayFlow's FraudShield by providing automated real-time transaction fraud prevention for SMB and Mid-Market merchants.",
            "key_points": [
                "Direct overlap with PayFlow's FraudShield product line.",
                "High product impact requiring velocity checking investigation.",
            ],
            "relevant_signals": [
                {
                    "competitor": "Cashfree",
                    "headline": "Launch of 'RiskShield', a real-time risk management solution for cyber payment frauds.",
                }
            ],
            "evidence": {
                "source_urls": [
                    "https://cashfree.com/riskshield-announcement",
                ]
            },
            "confidence": "high",
            "limitations": ["Technical false-positive benchmarks not verified in public release."],
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
                        "/api/ask",
                        json={
                            "question": "Why does Cashfree's RiskShield launch matter to PayFlow?",
                            "signals": [s.model_dump() for s in sample_signals],
                        },
                    )

    assert resp.status_code == 200
    data = resp.json()
    assert "directly competes with PayFlow's FraudShield" in data["answer"]
    assert len(data["key_points"]) == 2
    assert data["relevant_signals"][0]["competitor"] == "Cashfree"
    assert data["evidence"]["source_urls"] == ["https://cashfree.com/riskshield-announcement"]
    assert data["confidence"] == "high"


@pytest.mark.asyncio
async def test_ask_dronahq_json_string_response(sample_signals):
    """3. Test DronaHQ stringified JSON response handling."""
    structured = {
        "answer": "Both Cashfree and Razorpay are emphasizing real-time fraud mitigation.",
        "key_points": [
            "Cashfree introduced RiskShield.",
            "Razorpay rolled out Chargeback Shield.",
        ],
        "relevant_signals": [
            {"competitor": "Cashfree", "headline": "Launch of 'RiskShield'"},
            {"competitor": "Razorpay", "headline": "Introduction of Chargeback Shield"},
        ],
        "evidence": {
            "source_urls": [
                "https://cashfree.com/riskshield-announcement",
                "https://razorpay.com/chargeback-shield",
            ]
        },
        "confidence": "high",
        "limitations": [],
    }
    mock_dronahq_response = {
        "success": True,
        "response": json.dumps(structured),
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                res = await dronahq.ask_warroom(
                    question="Summarize the competitive landscape.",
                    signals=sample_signals,
                )

    assert "Both Cashfree and Razorpay" in res.answer
    assert len(res.key_points) == 2
    assert len(res.relevant_signals) == 2
    assert "https://razorpay.com/chargeback-shield" in res.evidence.source_urls


@pytest.mark.asyncio
async def test_ask_dronahq_markdown_fenced_response(sample_signals):
    """4. Test DronaHQ markdown-fenced JSON response handling."""
    fenced = (
        "```json\n"
        "{\n"
        '  "answer": "Sales should focus on PayFlow\'s integrated reconciliation advantage.",\n'
        '  "key_points": ["Counter Cashfree with native chargeback protection."],\n'
        '  "relevant_signals": [{"competitor": "Cashfree"}],\n'
        '  "evidence": {"source_urls": ["https://cashfree.com/riskshield-announcement"]},\n'
        '  "confidence": "medium",\n'
        '  "limitations": ["Objection patterns subject to prospect size."]\n'
        "}\n"
        "```"
    )
    mock_dronahq_response = {
        "success": True,
        "response": fenced,
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                res = await dronahq.ask_warroom(
                    question="What should sales investigate regarding Cashfree?",
                    signals=sample_signals,
                )

    assert "integrated reconciliation advantage" in res.answer
    assert res.confidence == "medium"
    assert res.evidence.source_urls == ["https://cashfree.com/riskshield-announcement"]


@pytest.mark.asyncio
async def test_ask_empty_and_whitespace_question_validation(sample_signals):
    """5. Test validation rejecting empty or whitespace-only questions with HTTP 422."""
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as ac:
        # Empty string
        resp1 = await ac.post(
            "/api/ask",
            json={"question": "", "signals": [s.model_dump() for s in sample_signals]},
        )
        assert resp1.status_code == 422

        # Whitespace only
        resp2 = await ac.post(
            "/api/ask",
            json={"question": "   \n\t  ", "signals": [s.model_dump() for s in sample_signals]},
        )
        assert resp2.status_code == 422


@pytest.mark.asyncio
async def test_ask_oversized_question_validation(sample_signals):
    """6. Test validation rejecting oversized questions (>1000 characters) with HTTP 422."""
    oversized = "a" * 1001
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as ac:
        resp = await ac.post(
            "/api/ask",
            json={"question": oversized, "signals": [s.model_dump() for s in sample_signals]},
        )
        assert resp.status_code == 422


@pytest.mark.asyncio
async def test_ask_dronahq_http_error_fallback(sample_signals):
    """7. Test DronaHQ HTTP error triggering structured fallback without crashing."""
    def handler(request: httpx.Request):
        return httpx.Response(502, text="Bad Gateway")

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                res = await dronahq.ask_warroom(
                    question="What are competitors doing in fraud?",
                    signals=sample_signals,
                )

    assert "AI reasoning is currently unavailable" in res.answer
    assert res.confidence == "low"
    assert len(res.key_points) > 0
    assert any("Cashfree" in p for p in res.key_points)
    assert any("502" in l or "AI reasoning service unavailable" in l for l in res.limitations)


@pytest.mark.asyncio
async def test_ask_dronahq_timeout_fallback(sample_signals):
    """8. Test DronaHQ timeout triggering fallback without crashing."""
    def handler(request: httpx.Request):
        raise httpx.TimeoutException("Connection timed out")

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                res = await dronahq.ask_warroom(
                    question="Why does this move matter?",
                    signals=sample_signals,
                )

    assert "AI reasoning is currently unavailable" in res.answer
    assert res.confidence == "low"
    assert len(res.evidence.source_urls) > 0


@pytest.mark.asyncio
async def test_ask_malformed_response_fallback(sample_signals):
    """9. Test DronaHQ returning an unparseable malformed response."""
    mock_dronahq_response = {
        "success": True,
        "response": "No json here, just an error string.",
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                res = await dronahq.ask_warroom(
                    question="What are the key moves?",
                    signals=sample_signals,
                )

    # Must return structured response gracefully
    assert res.confidence in ("low", "medium")
    assert len(res.answer) > 0
    assert len(res.relevant_signals) > 0


@pytest.mark.asyncio
async def test_ask_empty_signals_insufficient_context():
    """10. Test query with empty signals returns immediate insufficient context response."""
    res = await dronahq.ask_warroom(
        question="What are our competitors launching?",
        signals=[],
    )

    assert "Insufficient context" in res.answer
    assert res.confidence == "low"
    assert res.evidence.source_urls == []
    assert any("No active competitive signals" in l for l in res.limitations)


@pytest.mark.asyncio
async def test_ask_evidence_url_filtering(sample_signals):
    """11. Test that DronaHQ cannot hallucinate unsupplied evidence URLs."""
    mock_dronahq_response = {
        "success": True,
        "response": {
            "answer": "Here is an answer citing unverified sources.",
            "key_points": ["Point A"],
            "relevant_signals": [{"competitor": "Cashfree"}],
            "evidence": {
                "source_urls": [
                    "https://cashfree.com/riskshield-announcement",  # Valid supplied URL
                    "https://fake-news.com/fabricated-article",       # Hallucinated URL
                    "https://random-blog.org/competitor-leak",        # Hallucinated URL
                ]
            },
            "confidence": "high",
            "limitations": [],
        },
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                res = await dronahq.ask_warroom(
                    question="What evidence supports this?",
                    signals=sample_signals,
                )

    # The fabricated URLs MUST be stripped out
    assert "https://fake-news.com/fabricated-article" not in res.evidence.source_urls
    assert "https://random-blog.org/competitor-leak" not in res.evidence.source_urls
    # The valid supplied URL must be retained
    assert "https://cashfree.com/riskshield-announcement" in res.evidence.source_urls


@pytest.mark.asyncio
async def test_ask_relevant_signal_mapping_and_confidence():
    """12 & 13. Test relevant signal mapping and confidence when sources are empty."""
    no_source_signal = CompetitiveSignal(
        competitor="PayU",
        category="pricing",
        headline="PayU announces merchant discount adjustments",
        source_urls=[],
    )

    def handler(request: httpx.Request):
        return httpx.Response(500, text="Offline")

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                res = await dronahq.ask_warroom(
                    question="What did PayU announce?",
                    signals=[no_source_signal],
                )

    # Signal without source URLs must force confidence="low" and report limitation
    assert res.confidence == "low"
    assert res.evidence.source_urls == []
    assert any("Insufficient evidence" in l or "AI reasoning" in l for l in res.limitations)
    assert any(ref.competitor == "PayU" for ref in res.relevant_signals)
