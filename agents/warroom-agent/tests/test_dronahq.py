import pytest
import httpx
from unittest.mock import patch
from server import dronahq
from server.schemas import CompetitorResearch, ResearchResult


@pytest.fixture
def sample_research():
    return [
        CompetitorResearch(
            competitor="Cashfree",
            results=[
                ResearchResult(
                    title="Cashfree RiskShield Launch",
                    url="https://cashfree.com/riskshield",
                    snippet="Cyber payment fraud protection launched.",
                )
            ],
        ),
        CompetitorResearch(
            competitor="Razorpay",
            results=[
                ResearchResult(
                    title="Razorpay Ray AI",
                    url="https://razorpay.com/ray",
                    snippet="Conversational AI manager for settlements.",
                )
            ],
        ),
    ]


@pytest.mark.asyncio
async def test_dronahq_response_with_json_string(sample_research):
    """Test DronaHQ Standard response where 'response' is a stringified JSON object."""
    mock_dronahq_response = {
        "success": True,
        "thread_id": "thread_abc",
        "run_id": "run_xyz",
        "message": "Agent run completed successfully.",
        "response": (
            '{"status": "success", "signals": ['
            '{"competitor": "Cashfree", "headline": "Cashfree Launches RiskShield", '
            '"summary": "Cashfree introduced RiskShield to curb real-time payment fraud.", '
            '"category": "product", "source_urls": ["https://cashfree.com/riskshield"]}'
            ']}'
        ),
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                run, signals = await dronahq.trigger_reasoning({}, research_context=sample_research)

    assert run.status == "completed"
    assert run.thread_id == "thread_abc"
    assert run.run_id == "run_xyz"
    assert len(signals) == 1
    assert signals[0].competitor == "Cashfree"
    assert signals[0].headline == "Cashfree Launches RiskShield"
    assert signals[0].source_urls == ["https://cashfree.com/riskshield"]


@pytest.mark.asyncio
async def test_dronahq_response_with_parsed_object(sample_research):
    """Test DronaHQ Standard response where 'response' is an already-decoded dict."""
    mock_dronahq_response = {
        "success": True,
        "thread_id": "thread_abc",
        "run_id": "run_xyz",
        "message": "Agent run completed successfully.",
        "response": {
            "status": "success",
            "signals": [
                "Razorpay: Deployed RAY AI account manager on WhatsApp for automated settlement tracking."
            ],
        },
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                run, signals = await dronahq.trigger_reasoning({}, research_context=sample_research)

    assert run.status == "completed"
    assert len(signals) == 1
    assert signals[0].competitor == "Razorpay"
    assert "Razorpay" in signals[0].headline
    assert signals[0].source_urls == ["https://razorpay.com/ray"]


@pytest.mark.asyncio
async def test_dronahq_insufficient_evidence():
    """Test DronaHQ response when status is insufficient_evidence."""
    mock_dronahq_response = {
        "success": True,
        "thread_id": "thread_1",
        "run_id": "run_1",
        "message": "Agent completed with insufficient evidence.",
        "response": {
            "status": "insufficient_evidence",
            "signals": [],
        },
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                run, signals = await dronahq.trigger_reasoning({})

    assert run.status == "insufficient_evidence"
    assert signals == []


@pytest.mark.asyncio
async def test_dronahq_response_with_competitive_signals_markdown(sample_research):
    """Test DronaHQ response where 'response' is markdown-wrapped JSON with 'competitive_signals'."""
    raw_markdown = """```json
{
  "competitive_signals": [
    {
      "competitor": "Razorpay",
      "change": "Launched Ray conversational AI for automated payment operations",
      "signal_classification": {
        "type": "Product Launch",
        "threat_level": "High"
      },
      "significance_explanation": "Razorpay Ray allows enterprise clients to automate reconciliation and track dispute settlements.",
      "affected_internal_products": ["Payment Gateway", "Reconciliation"]
    }
  ]
}
```"""
    mock_dronahq_response = {
        "success": True,
        "thread_id": "thread_markdown_123",
        "run_id": "run_markdown_456",
        "message": "Agent run completed successfully. See 'response' for execution output.",
        "response": raw_markdown,
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                run, signals = await dronahq.trigger_reasoning({}, research_context=sample_research)

    assert run.status == "completed"
    assert run.thread_id == "thread_markdown_123"
    assert len(signals) == 1
    assert signals[0].competitor == "Razorpay"
    assert "Launched Ray conversational AI" in signals[0].headline
    assert "reconciliation" in signals[0].summary.lower()
    assert signals[0].source_urls == ["https://razorpay.com/ray"]


@pytest.mark.asyncio
async def test_dronahq_malformed_response(sample_research):
    """Test DronaHQ response when 'response' is malformed plain text rather than JSON."""
    mock_dronahq_response = {
        "success": True,
        "thread_id": "thread_malformed",
        "run_id": "run_malformed",
        "message": "Agent run completed.",
        "response": "Cashfree has recently expanded its presence in international payments and fraud detection.",
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                run, signals = await dronahq.trigger_reasoning({}, research_context=sample_research)

    assert run.status == "completed"
    assert len(signals) == 1
    assert signals[0].competitor == "Cashfree"
    assert "Cashfree" in signals[0].headline


@pytest.mark.asyncio
async def test_dronahq_rich_signal_parsing(sample_research):
    """Test that full rich intelligence fields are parsed and normalized."""
    mock_dronahq_response = {
        "success": True,
        "thread_id": "thread_rich_123",
        "run_id": "run_rich_456",
        "message": "Agent run completed successfully.",
        "response": {
            "status": "success",
            "competitive_signals": [
                {
                    "competitor": "Cashfree",
                    "change": "Launch of RiskShield real-time fraud prevention engine",
                    "signal_classification": {
                        "type": "product",
                        "significance": "SIGNIFICANT"
                    },
                    "significance_explanation": "Reduces fraudulent activities by up to 40% using machine learning.",
                    "affected_products": ["FraudShield", "Payment Gateway"],
                    "affected_segments": ["SMB", "Mid-Market"],
                    "impact_assessment": {
                        "product": "HIGH",
                        "sales": "MEDIUM",
                        "marketing": "MEDIUM",
                        "pricing": "LOW"
                    },
                    "product_investigation_recommendation": "Investigate enhancing PayFlow FraudShield feature parity.",
                    "sales_battlecard": {
                        "recommended_talk_track": "Emphasize PayFlow dedicated support and seamless integration."
                    },
                    "confidence": "high"
                }
            ]
        }
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                run, signals = await dronahq.trigger_reasoning({}, research_context=sample_research)

    assert run.status == "completed"
    assert len(signals) == 1
    sig = signals[0]
    assert sig.competitor == "Cashfree"
    assert sig.significance == "SIGNIFICANT"
    assert sig.overlap.products == ["FraudShield", "Payment Gateway"]
    assert sig.overlap.customer_segments == ["SMB", "Mid-Market"]
    assert sig.impact.product == "HIGH"
    assert sig.impact.sales == "MEDIUM"
    assert sig.impact.marketing == "MEDIUM"
    assert sig.impact.strategy == "LOW"
    assert len(sig.recommended_actions) == 2
    assert "Investigate enhancing" in sig.recommended_actions[0]
    assert "Sales talk track:" in sig.recommended_actions[1]
    assert sig.confidence == "high"
    assert sig.source_urls == ["https://cashfree.com/riskshield"]


@pytest.mark.asyncio
async def test_dronahq_minimal_signal_with_defaults(sample_research):
    """Test that a signal missing all optional rich fields safely defaults."""
    mock_dronahq_response = {
        "success": True,
        "thread_id": "thread_min",
        "run_id": "run_min",
        "message": "Agent run completed.",
        "response": {
            "status": "success",
            "signals": [
                {
                    "competitor": "Razorpay",
                    "headline": "Simple update",
                    "summary": "Summary text"
                }
            ]
        }
    }

    def handler(request: httpx.Request):
        return httpx.Response(200, json=mock_dronahq_response)

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                run, signals = await dronahq.trigger_reasoning({}, research_context=sample_research)

    assert run.status == "completed"
    assert len(signals) == 1
    sig = signals[0]
    assert sig.competitor == "Razorpay"
    assert sig.headline == "Simple update"
    assert sig.significance is None
    assert sig.overlap.products == []
    assert sig.overlap.customer_segments == []
    assert sig.impact.product is None
    assert sig.recommended_actions == []
    assert sig.confidence == "high"  # Sources exist for Razorpay
    assert sig.source_urls == ["https://razorpay.com/ray"]


@pytest.mark.asyncio
async def test_dronahq_http_error():
    """Test DronaHQ HTTP error raises DronaHQAPIError."""
    def handler(request: httpx.Request):
        return httpx.Response(500, text="Internal Server Error")

    transport = httpx.MockTransport(handler)

    with patch("server.config.settings.dronahq_webhook_url", "https://mock.dronahq.com/webhook/test"):
        with patch("server.config.settings.dronahq_api_key", "mock_key"):
            with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=transport)):
                with pytest.raises(dronahq.DronaHQAPIError):
                    await dronahq.trigger_reasoning({})


