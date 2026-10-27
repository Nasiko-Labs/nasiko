import pytest
from unittest.mock import patch
from httpx import ASGITransport, AsyncClient
from server.main import app
from server.schemas import ResearchResult, DronaHQRun, CompetitiveSignal


@pytest.mark.asyncio
async def test_scan_endpoint_processes_competitors_and_wires_signals():
    """Verify that /api/scan processes all competitors, preserves research,
    dispatches the combined payload to DronaHQ, and wires returned signals."""

    async def mock_anakin_search(prompt: str, limit: int = 3):
        competitor_name = prompt.split()[0]
        return [
            ResearchResult(
                title=f"{competitor_name} Official Update",
                url=f"https://{competitor_name.lower()}.com/news",
                snippet=f"Verified real public update from {competitor_name}.",
                date="2026-09-01",
                last_updated=None,
            )
        ]

    captured_dronahq_payload = {}

    async def mock_dronahq_trigger(payload: dict, research_context=None):
        nonlocal captured_dronahq_payload
        captured_dronahq_payload = payload
        run = DronaHQRun(
            status="completed",
            thread_id="thread_test_999",
            run_id="run_test_888",
            message="Agent run completed successfully.",
        )
        signals = [
            CompetitiveSignal(
                competitor="Cashfree",
                headline="Cashfree launches RiskShield",
                summary="Cyber payment fraud protection launched.",
                category="product",
                source_urls=["https://cashfree.com/news"],
            )
        ]
        return run, signals

    with patch("server.anakin.search", side_effect=mock_anakin_search):
        with patch("server.dronahq.trigger_reasoning", side_effect=mock_dronahq_trigger):
            transport = ASGITransport(app=app)
            async with AsyncClient(transport=transport, base_url="http://test") as client:
                response = await client.post("/api/scan", json={})

    assert response.status_code == 200
    data = response.json()

    # 1. Verify default company context (PayFlow)
    assert data["company"]["name"] == "PayFlow"
    assert data["company"]["products"] == ["Payment Gateway", "Reconciliation", "FraudShield"]
    assert data["company"]["target_customers"] == ["SMB", "Mid-Market"]

    # 2. Verify all three default competitors are researched
    competitors_researched = [r["competitor"] for r in data["research"]]
    assert competitors_researched == ["Cashfree", "Razorpay", "PayU"]

    # 3. Verify research integrity (snippets and URLs preserved exactly)
    for comp_res in data["research"]:
        assert len(comp_res["results"]) == 1
        res = comp_res["results"][0]
        assert comp_res["competitor"] in res["title"]
        assert comp_res["competitor"].lower() in res["url"]
        assert "Verified real public update" in res["snippet"]

    # 4. Verify combined payload sent to DronaHQ contains message and competitor data
    assert "Analyze the following competitor research" in captured_dronahq_payload["message"]
    assert captured_dronahq_payload["company"]["name"] == "PayFlow"
    assert len(captured_dronahq_payload["competitors"]) == 3

    # 5. Verify reasoning status is 'completed'
    assert data["reasoning"]["status"] == "completed"
    assert data["reasoning"]["thread_id"] == "thread_test_999"
    assert data["reasoning"]["run_id"] == "run_test_888"

    # 6. Verify signals are populated from DronaHQ with rich model structure
    assert len(data["signals"]) == 1
    assert data["signals"][0]["competitor"] == "Cashfree"
    assert data["signals"][0]["headline"] == "Cashfree launches RiskShield"
    assert "overlap" in data["signals"][0]
    assert "impact" in data["signals"][0]
    assert "recommended_actions" in data["signals"][0]


@pytest.mark.asyncio
async def test_scan_endpoint_preserves_research_on_dronahq_failure():
    """Verify that if DronaHQ fails, collected Anakin research is preserved."""
    async def mock_anakin_search(prompt: str, limit: int = 3):
        return [
            ResearchResult(
                title="Update",
                url="https://example.com",
                snippet="Snippet",
                date=None,
                last_updated=None,
            )
        ]

    from server.dronahq import DronaHQTimeoutError

    async def mock_dronahq_trigger(payload: dict, research_context=None):
        raise DronaHQTimeoutError("DronaHQ timed out")

    with patch("server.anakin.search", side_effect=mock_anakin_search):
        with patch("server.dronahq.trigger_reasoning", side_effect=mock_dronahq_trigger):
            transport = ASGITransport(app=app)
            async with AsyncClient(transport=transport, base_url="http://test") as client:
                response = await client.post("/api/scan", json={})

    assert response.status_code == 200
    data = response.json()
    assert len(data["research"]) == 3
    assert data["reasoning"]["status"] == "error"
    assert "AI reasoning unavailable" in data["reasoning"]["message"]
    assert data["signals"] == []
