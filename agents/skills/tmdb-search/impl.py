import os

import httpx


def tmdb_search(query: str, media_type: str = "movie", max_results: int = 5) -> str:
    """Search TMDB for movies or TV shows by title or keyword. media_type: 'movie' or 'tv'. Returns titles, ratings, release years, and overviews."""
    api_key = os.getenv("TMDB_API_KEY", "")
    if not api_key:
        return (
            "Error: TMDB_API_KEY environment variable not set. "
            "Get a free key at https://www.themoviedb.org/settings/api"
        )

    endpoint = "movie" if media_type == "movie" else "tv"
    resp = httpx.get(
        f"https://api.themoviedb.org/3/search/{endpoint}",
        params={"query": query, "api_key": api_key, "page": 1},
        timeout=10.0,
        follow_redirects=True,
    )
    if resp.status_code != 200:
        return f"TMDB API error {resp.status_code}: {resp.text[:200]}"

    results = resp.json().get("results", [])[:max_results]
    if not results:
        return f"No {media_type} results found for '{query}'."

    lines = [f"TMDB results for '{query}' ({media_type}):"]
    for r in results:
        title = r.get("title") or r.get("name", "Unknown")
        date = r.get("release_date") or r.get("first_air_date", "")
        year_str = date[:4] if date else "N/A"
        rating = r.get("vote_average", 0.0)
        votes = r.get("vote_count", 0)
        overview = (r.get("overview") or "No description available.")[:250]
        lines.append(
            f"\n• {title} ({year_str}) — ⭐ {rating:.1f}/10  ({votes} votes)\n  {overview}"
        )
    return "\n".join(lines)
