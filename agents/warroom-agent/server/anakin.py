import httpx
from server.config import settings
from server.schemas import ResearchResult

ANAKIN_SEARCH_URL = "https://api.anakin.io/v1/search"


class AnakinError(Exception):
    """Base exception for Anakin client operations."""


class AnakinConfigurationError(AnakinError):
    """Raised when Anakin credentials are not properly configured."""


class AnakinTimeoutError(AnakinError):
    """Raised when Anakin search request times out."""


class AnakinAPIError(AnakinError):
    """Raised when Anakin API returns an HTTP error status."""

    def __init__(self, status_code: int, detail: str):
        super().__init__(f"Anakin API returned {status_code}: {detail}")
        self.status_code = status_code
        self.detail = detail


class AnakinResponseError(AnakinError):
    """Raised when Anakin response cannot be parsed or format is invalid."""


async def search(prompt: str, limit: int = 3) -> list[ResearchResult]:
    """Execute live synchronous web search via Anakin Search API.

    Verified endpoint: POST https://api.anakin.io/v1/search
    Payload format: {"prompt": str, "limit": int}
    """
    api_key = settings.anakin_api_key
    if not api_key or not api_key.strip():
        raise AnakinConfigurationError("ANAKIN_API_KEY is not configured in the environment")

    headers = {
        "Content-Type": "application/json",
        "X-API-Key": api_key.strip(),
    }
    payload = {
        "prompt": prompt,
        "limit": limit,
    }

    try:
        async with httpx.AsyncClient(timeout=20.0) as client:
            response = await client.post(ANAKIN_SEARCH_URL, headers=headers, json=payload)
    except httpx.TimeoutException as exc:
        raise AnakinTimeoutError(f"Anakin search timed out after 20.0s for prompt: {prompt}") from exc
    except httpx.RequestError as exc:
        raise AnakinAPIError(status_code=500, detail=f"Network error communicating with Anakin: {exc}") from exc

    if response.status_code != 200:
        raise AnakinAPIError(status_code=response.status_code, detail=response.text)

    try:
        data = response.json()
    except Exception as exc:
        raise AnakinResponseError(f"Invalid JSON response received from Anakin: {exc}") from exc

    if not isinstance(data, dict):
        raise AnakinResponseError(f"Expected dictionary response from Anakin, got {type(data).__name__}")

    raw_results = data.get("results")
    if raw_results is None:
        raise AnakinResponseError("Missing 'results' field in Anakin search response")
    if not isinstance(raw_results, list):
        raise AnakinResponseError(f"Expected 'results' to be a list, got {type(raw_results).__name__}")

    parsed_results: list[ResearchResult] = []
    for item in raw_results:
        if not isinstance(item, dict):
            continue

        raw_date = item.get("date")
        clean_date = raw_date.strip() if isinstance(raw_date, str) and raw_date.strip() else None

        raw_updated = item.get("last_updated")
        clean_updated = (
            raw_updated.strip() if isinstance(raw_updated, str) and raw_updated.strip() else None
        )

        parsed_results.append(
            ResearchResult(
                title=str(item.get("title", "")),
                url=str(item.get("url", "")),
                snippet=str(item.get("snippet", "")),
                date=clean_date,
                last_updated=clean_updated,
            )
        )

    return parsed_results
