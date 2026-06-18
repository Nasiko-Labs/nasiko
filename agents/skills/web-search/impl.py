import httpx


def web_search(query: str, max_results: int = 5) -> str:
    """Search the web for information on any topic using DuckDuckGo. Returns up to max_results summaries."""
    resp = httpx.get(
        "https://api.duckduckgo.com/",
        params={"q": query, "format": "json", "no_html": "1", "skip_disambig": "1"},
        timeout=10.0,
        follow_redirects=True,
    )
    if resp.status_code != 200:
        return f"Search failed for '{query}'."
    data = resp.json()
    parts: list[str] = []
    if data.get("AbstractText"):
        parts.append(data["AbstractText"])
    for topic in data.get("RelatedTopics", [])[:max_results]:
        if isinstance(topic, dict) and topic.get("Text"):
            parts.append(f"• {topic['Text']}")
    return "\n".join(parts) if parts else f"No results found for '{query}'."
