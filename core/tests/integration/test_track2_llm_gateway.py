import pytest
import requests

GATEWAY_URL = "http://localhost:4001"
VIRTUAL_KEY = "sk-nasiko-gateway-key"
HEADERS = {"Authorization": f"Bearer {VIRTUAL_KEY}"}


def test_gateway_is_running():
    """Track 2 Test 1: Boot platform -> gateway is up and reachable"""
    response = requests.get(f"{GATEWAY_URL}/health", headers=HEADERS)
    assert response.status_code == 200
    data = response.json()
    assert data["healthy_count"] >= 1


def test_gateway_model_list():
    """Track 2 Test 2: Gateway exposes correct models"""
    response = requests.get(f"{GATEWAY_URL}/v1/models", headers=HEADERS)
    assert response.status_code == 200
    models = response.json()["data"]
    model_ids = [m["id"] for m in models]
    assert "gpt-4o-mini" in model_ids


def test_gateway_llm_call():
    """Track 2 Test 3: Sample agent completes LLM call via gateway"""
    response = requests.post(
        f"{GATEWAY_URL}/v1/chat/completions",
        headers=HEADERS,
        json={
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "Say hello"}],
        },
    )
    assert response.status_code == 200
    data = response.json()
    assert "choices" in data
    assert len(data["choices"]) > 0


def test_gateway_rejects_invalid_key():
    """Track 2 Test 4: Gateway rejects invalid keys"""
    response = requests.get(
        f"{GATEWAY_URL}/health",
        headers={"Authorization": "Bearer wrong-key"}
    )
    assert response.status_code in [400, 401]
