import httpx


def http_request(url: str, method: str = "GET", body: str = "") -> str:
    """Make an HTTP request to any URL. method: GET, POST, PUT, DELETE, PATCH. Returns HTTP status and response body (truncated to 3000 chars)."""
    kwargs: dict = {"timeout": 15.0, "follow_redirects": True}
    if body:
        kwargs["content"] = body.encode()
    resp = httpx.request(method.upper(), url, **kwargs)
    return f"HTTP {resp.status_code}\n{resp.text[:3000]}"
