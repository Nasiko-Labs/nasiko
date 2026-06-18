import time
import xml.etree.ElementTree as ET

import httpx

_SORT_MAP = {
    "newest": "submittedDate",
    "updated": "lastUpdatedDate",
    "relevance": "relevance",
    "cited": "relevance",
}
_NS = {"atom": "http://www.w3.org/2005/Atom"}
_HEADERS = {
    "User-Agent": "NasikoAgent/1.0 (https://nasiko.com; research tool)",
    "Accept": "application/atom+xml",
}


def arxiv_search(query: str, max_results: int = 5, sort_by: str = "newest") -> str:
    """Search arXiv for academic research papers by keyword, topic, or author. Returns titles, authors, abstracts, and links."""
    quoted = f'all:"{query}"' if " " in query else f"all:{query}"
    params = {
        "search_query": quoted,
        "max_results": min(max_results, 10),
        "sortBy": _SORT_MAP.get(sort_by, "submittedDate"),
        "sortOrder": "descending",
    }

    resp = None
    for attempt in range(2):
        try:
            resp = httpx.get(
                "https://export.arxiv.org/api/query",
                params=params,
                headers=_HEADERS,
                timeout=30.0,
                follow_redirects=True,
            )
        except httpx.TimeoutException:
            return "arXiv request timed out. The server may be busy — please try again shortly."
        except httpx.RequestError as exc:
            return f"Network error reaching arXiv: {exc}"

        if resp.status_code == 200:
            break
        if resp.status_code in (429, 503) and attempt == 0:
            time.sleep(3)
            continue
        return f"arXiv returned HTTP {resp.status_code}. Please try again in a few seconds."

    if resp is None or resp.status_code != 200:
        return "arXiv is temporarily unavailable. Please try again shortly."

    try:
        root = ET.fromstring(resp.text)
    except ET.ParseError:
        return "arXiv returned an unexpected response format."

    entries = root.findall("atom:entry", _NS)
    if not entries:
        return f"No papers found for '{query}'. Try broader keywords."

    lines = [f"arXiv papers for '{query}':"]
    for entry in entries:
        title = (entry.findtext("atom:title", "", _NS) or "").strip().replace("\n", " ")
        all_authors = entry.findall("atom:author", _NS)
        authors = [a.findtext("atom:name", "", _NS) for a in all_authors[:3]]
        summary = (entry.findtext("atom:summary", "", _NS) or "").strip().replace("\n", " ")[:300]
        link = (entry.findtext("atom:id", "", _NS) or "").strip()
        published = (entry.findtext("atom:published", "", _NS) or "")[:10]

        author_str = ", ".join(a for a in authors if a)
        if len(all_authors) > 3:
            author_str += " et al."

        lines.append(f"\n• {title}")
        if author_str:
            lines.append(f"  Authors: {author_str}")
        lines.append(f"  Published: {published}  |  {link}")
        if summary:
            lines.append(f"  {summary}…")

    return "\n".join(lines)
