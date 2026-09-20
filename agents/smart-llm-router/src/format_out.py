"""Console reply = chat only. Analytics never go in the Nasiko message."""

from __future__ import annotations

import re

from router_client import RouterError, RouterSuccess


_GREETINGS = frozenset(
    {"hi", "hello", "hey", "yo", "sup", "hola", "thanks", "thank you", "ok", "okay"}
)


def format_chat_reply(router: RouterSuccess, prompt: str = "") -> str:
    """Plain assistant text for the Nasiko console / A2A client."""
    body = (router.content or "").strip()
    # Stub router returns a labeled fake line — never show the raw `[STUB · …]` tag.
    if body.startswith("[STUB"):
        p = re.sub(r"[!?.]+$", "", (prompt or "").strip().lower())
        if p in _GREETINGS:
            return "Hi! How can I help you today?"
        return (
            "The router is in stub mode (NASIKO_ROUTE_STUB), so I can’t answer that yet. "
            "Turn stub off and retry for a real model reply."
        )
    return body or "(empty reply)"


def format_error(err: RouterError) -> str:
    """Short user-facing error — no stack traces or raw provider dumps."""
    msg = (err.message or "").strip()
    # Prefer a short, readable line over JSON blobs.
    if "Incorrect API key" in msg or "invalid_api_key" in msg:
        return "The language model key isn’t configured correctly. Please ask an admin to check the server keys."
    if "Authorization failed" in msg or "403" in msg or "401" in msg:
        return (
            "No working model provider key is configured "
            "(OpenAI/Groq/NVIDIA all failed auth). "
            "Add a valid key in the server .env and retry."
        )
    if err.code == "timeout":
        return "The model took too long to respond. Please try again."
    if len(msg) > 160:
        msg = msg[:157] + "…"
    return f"Sorry — I couldn’t complete that ({err.code}). {msg}".strip()
