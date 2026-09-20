"""Translation tools — Google Translate API + BeautifulSoup web extraction."""

import asyncio
import logging
from urllib.parse import urlparse

import requests
from bs4 import BeautifulSoup
from langdetect import DetectorFactory, detect

DetectorFactory.seed = 0
logger = logging.getLogger(__name__)

_SESSION = requests.Session()
_SESSION.headers.update({
    "User-Agent": (
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
        "AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36"
    )
})


def _google_translate(text: str, src: str, dest: str) -> tuple[str, str]:
    """Call the Google Translate unofficial API. Returns (translated_text, detected_src)."""
    resp = _SESSION.get(
        "https://translate.googleapis.com/translate_a/single",
        params={"client": "gtx", "sl": src, "tl": dest, "dt": "t", "q": text},
        timeout=10,
    )
    resp.raise_for_status()
    data = resp.json()
    translated = "".join(s[0] for s in data[0] if s and s[0])
    detected_src = data[2] if len(data) > 2 and data[2] else src
    return translated.strip(), detected_src


def _extract_url_text(url: str) -> tuple[str, str | None]:
    """Fetch a web page and return (clean_text, page_title)."""
    parsed = urlparse(url)
    if not parsed.scheme or not parsed.netloc:
        raise ValueError(f"Invalid URL: {url}")
    resp = _SESSION.get(url, timeout=10)
    resp.raise_for_status()
    soup = BeautifulSoup(resp.content, "html.parser")
    for tag in soup(["script", "style"]):
        tag.decompose()
    title_tag = soup.find("title")
    title = title_tag.get_text().strip() if title_tag else None
    body = soup.find("body") or soup
    lines = (line.strip() for line in body.get_text().splitlines())
    text = " ".join(chunk for line in lines for chunk in line.split("  ") if chunk.strip())
    return text, title


def _detect_lang(text: str) -> tuple[str, float]:
    try:
        return detect(text[:1000]), 0.9
    except Exception:
        return "unknown", 0.0


class TranslatorToolset:
    """Callable tool implementations for OpenAI function calling."""

    async def translate_text(
        self,
        text: str,
        target_language: str = "en",
        source_language: str | None = None,
    ) -> str:
        """Translate plain text from one language to another.

        Args:
            text: Text to translate.
            target_language: BCP-47 language code of the target language (e.g. 'es', 'fr').
            source_language: Source language code; omit to auto-detect.
        """
        if not text.strip():
            return "Error: empty text provided."
        src = source_language or _detect_lang(text)[0]
        loop = asyncio.get_event_loop()
        translated, detected_src = await loop.run_in_executor(
            None, _google_translate, text, src, target_language
        )
        return f"[{detected_src} → {target_language}] {translated}"

    def translate_url(
        self,
        url: str,
        target_language: str = "en",
        source_language: str | None = None,
    ) -> str:
        """Extract and translate content from a web page URL.

        Args:
            url: Web page URL.
            target_language: BCP-47 language code of the target language.
            source_language: Source language code; omit to auto-detect.
        """
        text, title = _extract_url_text(url)
        if not text.strip():
            return "Error: no readable text found on the page."
        if len(text) > 5000:
            text = text[:5000] + "…"
        src = source_language or _detect_lang(text)[0]
        translated, detected_src = _google_translate(text, src, target_language)
        prefix = f"[{title}] " if title else ""
        return f"{prefix}[{detected_src} → {target_language}] {translated}"

    def detect_language(
        self,
        text: str | None = None,
        url: str | None = None,
    ) -> str:
        """Detect the language of text or a web page.

        Args:
            text: Text to analyse (mutually exclusive with url).
            url: Web page URL to fetch and analyse.
        """
        if text and url:
            return "Error: provide either text or url, not both."
        if not text and not url:
            return "Error: provide text or url."
        if url:
            text, _ = _extract_url_text(url)
        if not text or not text.strip():
            return "Error: no text available for detection."
        lang, confidence = _detect_lang(text)
        return f"Detected language: {lang} (confidence: {confidence:.0%})"