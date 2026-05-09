"""Canonical-form helper for cache keying."""
import json
import re
from typing import Any

_WHITESPACE = re.compile(r"\s+")
_PUNCT_RUN = re.compile(r"([!?\.,;:])\1+")
_TRAILING_PUNCT = re.compile(r"[\s!?\.,;:]+$")


def _is_default_value(value: Any) -> bool:
    """Drop keys whose values carry no information."""

    return value is None or value == "" or value == [] or value == {}


def _canonicalize_json_value(value: Any) -> Any:
    """Recursively lowercase strings, sort dict keys, drop default values."""

    if isinstance(value, dict):
        cleaned = {}
        for key in sorted(value.keys()):
            child = _canonicalize_json_value(value[key])
            if _is_default_value(child):
                continue
            cleaned[key] = child
        return cleaned
    if isinstance(value, list):
        return [_canonicalize_json_value(item) for item in value]
    if isinstance(value, str):
        return _normalize_text(value)
    return value


def _normalize_text(text: str) -> str:
    """Apply the text-level normalization rules."""

    lowered = text.strip().lower()
    collapsed = _WHITESPACE.sub(" ", lowered)
    deduped = _PUNCT_RUN.sub(r"\1", collapsed)
    return _TRAILING_PUNCT.sub("", deduped)


def normalize(payload: bytes | str | dict[str, Any] | None) -> str:
    """Return a canonical string representation of ``payload``.

    Args:
        payload: A request body. Can be raw bytes, a JSON string, a parsed
            dict, or ``None`` (treated as an empty body).

    Returns:
        A canonical string suitable for hashing or embedding. Equivalent
        inputs produce equivalent outputs; non-equivalent inputs produce
        different outputs.
    """

    if payload is None:
        return ""

    if isinstance(payload, bytes):
        try:
            payload = payload.decode("utf-8")
        except UnicodeDecodeError:
            # Fall back to a literal byte representation rather than crash.
            return repr(payload)

    if isinstance(payload, str):
        stripped = payload.strip()
        if not stripped:
            return ""
        # Best-effort JSON parse — many agent endpoints accept JSON bodies.
        try:
            parsed = json.loads(stripped)
        except json.JSONDecodeError:
            return _normalize_text(stripped)
        return _canonical_json_string(parsed)

    if isinstance(payload, dict):
        return _canonical_json_string(payload)

    # Lists, ints, etc.
    return _canonical_json_string(payload)


def _canonical_json_string(value: Any) -> str:
    canonical = _canonicalize_json_value(value)
    return json.dumps(canonical, sort_keys=True, separators=(",", ":"))
