import pytest
import httpx
from unittest.mock import patch, MagicMock
from server import anakin


@pytest.mark.asyncio
async def test_anakin_search_request_format_and_parsing():
    """Verify that Anakin client sends {'prompt': '...', 'limit': 3} and parses results accurately."""
    mock_payload = {
        "id": "search_12345",
        "results": [
            {
                "title": "Cashfree Launches Tally Integration",
                "url": "https://cashfree.com/press/tally",
                "snippet": "Instant automated reconciliation directly in Tally ERP.",
                "date": "2026-09-01",
                "last_updated": "2026-09-02",
            },
            {
                "title": "Cashfree Payouts Documentation",
                "url": "https://cashfree.com/docs/payouts",
                "snippet": "API reference for bank reconciliation and vendor payouts.",
                "date": "",
                "last_updated": "",
            },
        ],
    }

    captured_request = {}

    def mock_handler(request: httpx.Request):
        nonlocal captured_request
        import json

        captured_request["url"] = str(request.url)
        captured_request["headers"] = dict(request.headers)
        captured_request["json"] = json.loads(request.content.decode("utf-8"))
        return httpx.Response(200, json=mock_payload)

    mock_transport = httpx.MockTransport(mock_handler)

    with patch("server.config.settings.anakin_api_key", "test_anakin_key"):
        with patch("httpx.AsyncClient", return_value=httpx.AsyncClient(transport=mock_transport)):
            results = await anakin.search(prompt="Cashfree Payment Gateway Reconciliation", limit=3)

    # 1. Verify exact request structure required by Anakin API
    assert captured_request["url"] == "https://api.anakin.io/v1/search"
    assert captured_request["headers"]["x-api-key"] == "test_anakin_key"
    assert captured_request["headers"]["content-type"] == "application/json"
    assert captured_request["json"] == {
        "prompt": "Cashfree Payment Gateway Reconciliation",
        "limit": 3,
    }
    # Ensure stale 'query' or 'num_results' are NOT present
    assert "query" not in captured_request["json"]
    assert "num_results" not in captured_request["json"]

    # 2. Verify parsed results
    assert len(results) == 2
    assert results[0].title == "Cashfree Launches Tally Integration"
    assert results[0].url == "https://cashfree.com/press/tally"
    assert results[0].snippet == "Instant automated reconciliation directly in Tally ERP."
    assert results[0].date == "2026-09-01"
    assert results[0].last_updated == "2026-09-02"

    # Empty string dates must be parsed as None
    assert results[1].date is None
    assert results[1].last_updated is None


@pytest.mark.asyncio
async def test_anakin_search_missing_api_key_raises_configuration_error():
    with patch("server.config.settings.anakin_api_key", None):
        with pytest.raises(anakin.AnakinConfigurationError) as exc_info:
            await anakin.search(prompt="Test prompt", limit=3)
        assert "ANAKIN_API_KEY is not configured" in str(exc_info.value)
