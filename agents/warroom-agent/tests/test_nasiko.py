import json
from pathlib import Path
from unittest.mock import AsyncMock, patch
import pytest
from httpx import ASGITransport, AsyncClient

from server.main import app
from server.schemas import (
    AskWarroomEvidence,
    AskWarroomResponse,
    CompanyContext,
    CompetitorResearch,
    DronaHQRun,
    RelevantSignalRef,
    ScanResponse,
)


@pytest.fixture
def agent_card_data():
    card_path = Path(__file__).resolve().parent.parent / "AgentCard.json"
    assert card_path.exists(), "AgentCard.json must exist in project root"
    with open(card_path, "r", encoding="utf-8") as f:
        return json.load(f)


def test_agent_card_schema_and_required_fields(agent_card_data):
    """Test 1: AgentCard exists and contains all fields required by Nasiko validate.rs."""
    required_fields = [
        "name",
        "description",
        "url",
        "version",
        "capabilities",
        "skills",
        "protocolVersion",
        "preferredTransport",
    ]
    for field in required_fields:
        assert field in agent_card_data, f"Missing required field: {field}"

    assert agent_card_data["name"] == "warroom-agent"
    assert agent_card_data["preferredTransport"] == "JSONRPC"
    assert agent_card_data["protocolVersion"] == "1.0"
    assert isinstance(agent_card_data["capabilities"], dict)
    assert agent_card_data["capabilities"].get("streaming") is False


def test_agent_card_skills(agent_card_data):
    """Test 2: AgentCard has exactly 3 WARROOM skills with valid schemas."""
    skills = agent_card_data.get("skills", [])
    assert len(skills) == 3, f"Expected 3 skills, got {len(skills)}"

    skill_ids = [s["id"] for s in skills]
    assert "competitive-scan" in skill_ids
    assert "response-simulate" in skill_ids
    assert "ask-warroom" in skill_ids

    for s in skills:
        assert "name" in s and s["name"]
        assert "description" in s and s["description"]
        assert "tags" in s and isinstance(s["tags"], list)
        assert "examples" in s and isinstance(s["examples"], list) and len(s["examples"]) > 0


@pytest.mark.asyncio
async def test_get_agent_card_well_known():
    """Test 3: GET /.well-known/agent-card.json returns 200 and matches AgentCard."""
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        resp = await client.get("/.well-known/agent-card.json")
        assert resp.status_code == 200
        data = resp.json()
        assert data["name"] == "warroom-agent"
        assert data["preferredTransport"] == "JSONRPC"
        assert len(data["skills"]) == 3


@pytest.mark.asyncio
async def test_get_agent_card_legacy_alias():
    """Test 4: GET /.well-known/agent.json returns 200."""
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        resp = await client.get("/.well-known/agent.json")
        assert resp.status_code == 200
        data = resp.json()
        assert data["name"] == "warroom-agent"


@pytest.mark.asyncio
async def test_a2a_invalid_jsonrpc_returns_error_32600():
    """Test 5: Invalid JSON-RPC returns error -32600."""
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        # Missing jsonrpc field
        resp = await client.post("/a2a", json={"id": "err-1", "method": "SendMessage"})
        assert resp.status_code == 200
        data = resp.json()
        assert data.get("error", {}).get("code") == -32600

        # Wrong jsonrpc version
        resp = await client.post("/a2a", json={"jsonrpc": "1.0", "id": "err-2", "method": "SendMessage"})
        assert resp.status_code == 200
        data = resp.json()
        assert data.get("error", {}).get("code") == -32600


@pytest.mark.asyncio
async def test_a2a_unknown_method_returns_error_32601():
    """Test 6: Unknown method returns error -32601."""
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        resp = await client.post("/a2a", json={
            "jsonrpc": "2.0",
            "id": "err-3",
            "method": "ExecuteUnknownTool",
            "params": {}
        })
        assert resp.status_code == 200
        data = resp.json()
        assert data.get("error", {}).get("code") == -32601
        assert "Method not found" in data["error"]["message"]


@pytest.mark.asyncio
async def test_a2a_send_message_routes_to_ask_warroom():
    """Test 7: SendMessage routes to Ask WARROOM and returns valid A2A 1.0 completed envelope."""
    transport = ASGITransport(app=app)
    mock_ask_resp = AskWarroomResponse(
        answer="Cashfree RiskShield directly threatens PayFlow FraudShield in the mid-market segment.",
        key_points=["Direct overlap with FraudShield", "Real-time AI monitoring"],
        relevant_signals=[RelevantSignalRef(competitor="Cashfree", headline="Launch of RiskShield")],
        evidence=AskWarroomEvidence(source_urls=["https://cashfree.com/riskshield"]),
        confidence="high",
        limitations=[],
    )

    with patch("server.dronahq.ask_warroom", new_callable=AsyncMock) as mock_ask:
        mock_ask.return_value = mock_ask_resp

        async with AsyncClient(transport=transport, base_url="http://test") as client:
            req = {
                "jsonrpc": "2.0",
                "id": "msg-001",
                "method": "SendMessage",
                "params": {
                    "message": {
                        "messageId": "client-msg-1",
                        "role": "ROLE_USER",
                        "parts": [
                            {"text": "Why does Cashfree RiskShield matter to PayFlow?"}
                        ]
                    }
                }
            }
            resp = await client.post("/a2a", json=req, headers={"A2A-Version": "1.0"})
            assert resp.status_code == 200
            data = resp.json()

            assert data["jsonrpc"] == "2.0"
            assert data["id"] == "msg-001"
            assert "result" in data
            task = data["result"]["task"]
            assert task["status"]["state"] == "TASK_STATE_COMPLETED"
            assert len(task["artifacts"]) > 0
            artifact_text = task["artifacts"][0]["parts"][0]["text"]
            assert "Cashfree RiskShield" in artifact_text
            assert "https://cashfree.com/riskshield" in artifact_text


@pytest.mark.asyncio
async def test_a2a_legacy_message_send_routes_to_ask_warroom():
    """Test 8: Legacy method 'message/send' routes to Ask WARROOM."""
    transport = ASGITransport(app=app)
    mock_ask_resp = AskWarroomResponse(
        answer="PayFlow maintains a competitive advantage through seamless smart routing.",
        key_points=["Smart routing is low cost"],
        relevant_signals=[],
        evidence=AskWarroomEvidence(source_urls=[]),
        confidence="medium",
        limitations=[],
    )

    with patch("server.dronahq.ask_warroom", new_callable=AsyncMock) as mock_ask:
        mock_ask.return_value = mock_ask_resp

        async with AsyncClient(transport=transport, base_url="http://test") as client:
            req = {
                "jsonrpc": "2.0",
                "id": "legacy-002",
                "method": "message/send",
                "params": {
                    "message": {
                        "parts": [
                            {"text": "What is PayFlow's primary advantage?"}
                        ]
                    }
                }
            }
            resp = await client.post("/a2a", json=req)
            assert resp.status_code == 200
            data = resp.json()
            assert data["id"] == "legacy-002"
            assert data["result"]["task"]["status"]["state"] == "TASK_STATE_COMPLETED"
            assert "smart routing" in data["result"]["task"]["artifacts"][0]["parts"][0]["text"]


@pytest.mark.asyncio
async def test_a2a_scan_intent_routes_to_scan():
    """Test 9: Scan intent ('run scan') routes to existing scan pipeline with mocking."""
    transport = ASGITransport(app=app)
    mock_scan_resp = ScanResponse(
        company=CompanyContext(name="PayFlow", products=["Payment Gateway"]),
        research=[CompetitorResearch(competitor="Cashfree", results=[])],
        reasoning=DronaHQRun(status="completed", message="Scan complete"),
        signals=[],
    )

    with patch("server.main.scan_competitors", new_callable=AsyncMock) as mock_scan:
        mock_scan.return_value = mock_scan_resp

        async with AsyncClient(transport=transport, base_url="http://test") as client:
            req = {
                "jsonrpc": "2.0",
                "id": "scan-001",
                "method": "SendMessage",
                "params": {
                    "message": {
                        "parts": [
                            {"text": "Run competitive scan against monitored targets"}
                        ]
                    }
                }
            }
            resp = await client.post("/a2a", json=req)
            assert resp.status_code == 200
            data = resp.json()
            assert data["id"] == "scan-001"
            assert data["result"]["task"]["status"]["state"] == "TASK_STATE_COMPLETED"
            assert "WARROOM Competitive Scan Report for PayFlow" in data["result"]["task"]["artifacts"][0]["parts"][0]["text"]
            mock_scan.assert_awaited_once()


@pytest.mark.asyncio
async def test_a2a_response_envelope_structure():
    """Test 10: Complete A2A 1.0 envelope verification (jsonrpc, id, task, state, artifacts)."""
    transport = ASGITransport(app=app)
    with patch("server.dronahq.ask_warroom", new_callable=AsyncMock) as mock_ask:
        mock_ask.return_value = AskWarroomResponse(
            answer="Landscape summary.",
            key_points=["Point 1"],
            evidence=AskWarroomEvidence(source_urls=[]),
            confidence="high",
            limitations=[],
        )
        async with AsyncClient(transport=transport, base_url="http://test") as client:
            req = {
                "jsonrpc": "2.0",
                "id": "env-001",
                "method": "SendMessage",
                "params": {"message": {"parts": [{"text": "Summarize market"}]}}
            }
            resp = await client.post("/a2a", json=req)
            data = resp.json()
            assert data["jsonrpc"] == "2.0"
            assert data["id"] == "env-001"
            task = data["result"]["task"]
            assert "id" in task and task["id"].startswith("task-")
            assert "contextId" in task and task["contextId"].startswith("ctx-")
            assert task["status"]["state"] == "TASK_STATE_COMPLETED"
            assert "timestamp" in task["status"]
            artifacts = task["artifacts"]
            assert isinstance(artifacts, list) and len(artifacts) == 1
            assert "artifactId" in artifacts[0]
            assert artifacts[0]["parts"][0]["text"] == "Landscape summary.\n\nKey Takeaways:\n- Point 1\n\nConfidence: high"


@pytest.mark.asyncio
async def test_existing_endpoints_regression():
    """Test 11: Regression test ensuring existing endpoints continue working undisturbed."""
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        # /api/health
        health_resp = await client.get("/api/health")
        assert health_resp.status_code == 200
        assert health_resp.json() == {"status": "ok", "service": "warroom-api"}

        # /api/ask empty signals
        ask_resp = await client.post("/api/ask", json={"question": "Test question", "signals": []})
        assert ask_resp.status_code == 200
        assert ask_resp.json()["confidence"] == "low"
        assert "signals" in ask_resp.json()["limitations"][0]

