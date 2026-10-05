import pytest
from httpx import ASGITransport, AsyncClient
from server.main import app


@pytest.mark.asyncio
async def test_health_check_returns_ok():
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        response = await client.get("/api/health")

    assert response.status_code == 200
    data = response.json()
    assert data == {
        "status": "ok",
        "service": "warroom-api",
    }
    # Ensure no secrets or unexpected keys are exposed
    assert "key" not in data
    assert "token" not in data
