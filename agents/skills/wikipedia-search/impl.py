import httpx


def search_wikipedia(query: str) -> str:
    """Search Wikipedia for a summary of any topic or concept."""
    resp = httpx.get(
        f"https://en.wikipedia.org/api/rest_v1/page/summary/{query.replace(' ', '_')}",
        follow_redirects=True,
        timeout=10.0,
    )
    if resp.status_code != 200:
        return f"No Wikipedia article found for '{query}'."
    data = resp.json()
    title = data.get("title", query)
    extract = data.get("extract", "No extract available.")
    return f"**{title}**\n\n{extract}"
