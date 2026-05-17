"""
VALSEA API client — transcription + sentiment.

Docs: https://valsea.ai/docs/api
"""

from __future__ import annotations

import base64
import io
import logging
import os
from typing import Optional, Tuple

import httpx

logger = logging.getLogger("callsense-agent.valsea")

DEFAULT_BASE_URL = "https://api.valsea.ai"
DEFAULT_LANGUAGE = "singlish"


def get_api_key() -> str:
    key = os.getenv("VALSEA_API_KEY") or os.getenv("VALSA_API_KEY")
    if not key:
        raise ValueError("Missing VALSEA_API_KEY or VALSA_API_KEY")
    return key


def get_base_url() -> str:
    return os.getenv("VALSEA_API_URL", DEFAULT_BASE_URL).rstrip("/")


def get_transcribe_url() -> str:
    return os.getenv(
        "VALSEA_TRANSCRIBE_URL",
        f"{get_base_url()}/v1/audio/transcriptions",
    )


def get_sentiment_url() -> str:
    return os.getenv(
        "VALSEA_SENTIMENT_URL",
        f"{get_base_url()}/v1/sentiment",
    )


def _auth_headers() -> dict[str, str]:
    return {"Authorization": f"Bearer {get_api_key()}"}


async def transcribe_audio_base64(
    audio_base64: str,
    *,
    language: Optional[str] = None,
    timeout: float = 60.0,
) -> str:
    """Transcribe base64 audio via VALSEA v1 API (multipart upload)."""
    lang = language or os.getenv("VALSEA_LANGUAGE", DEFAULT_LANGUAGE)
    audio_bytes = base64.b64decode(audio_base64, validate=False)
    if not audio_bytes:
        raise ValueError("audio_base64 decoded to empty bytes")

    v1_url = get_transcribe_url()

    async with httpx.AsyncClient(timeout=timeout) as client:
        files = {"file": ("audio.wav", io.BytesIO(audio_bytes), "audio/wav")}
        data = {
            "model": "valsea-transcribe",
            "language": lang,
            "response_format": "json",
        }
        try:
            response = await client.post(
                v1_url,
                headers=_auth_headers(),
                files=files,
                data=data,
            )
            response.raise_for_status()
            payload = response.json()
            text = payload.get("text") or payload.get("transcript")
            if text:
                logger.info("[VALSEA] v1 transcription ok (%d chars)", len(text))
                return str(text).strip()
        except httpx.HTTPError as exc:
            logger.warning("[VALSEA] v1 transcribe failed, trying legacy: %s", exc)

        legacy_url = f"{base}/transcribe"
        legacy_body = {"audio": audio_base64, "language": "en-SEA"}
        legacy_response = await client.post(
            legacy_url,
            headers={**_auth_headers(), "Content-Type": "application/json"},
            json=legacy_body,
        )
        legacy_response.raise_for_status()
        legacy_payload = legacy_response.json()
        text = (
            legacy_payload.get("transcript")
            or legacy_payload.get("text")
            or legacy_payload.get("result")
        )
        if not text:
            raise ValueError("VALSEA transcription returned no transcript")
        logger.info("[VALSEA] legacy transcription ok (%d chars)", len(str(text)))
        return str(text).strip()


async def analyze_sentiment(
    transcript: str,
    *,
    timeout: float = 30.0,
) -> Tuple[float, str]:
    """Return (confidence score, sentiment label) from VALSEA."""
    url = get_sentiment_url()

    async with httpx.AsyncClient(timeout=timeout) as client:
        try:
            response = await client.post(
                url,
                headers={**_auth_headers(), "Content-Type": "application/json"},
                json={"model": "valsea-sentiment", "transcript": transcript},
            )
            response.raise_for_status()
            payload = response.json()
            label = (
                payload.get("sentiment")
                or payload.get("sentiment_label")
                or payload.get("label")
                or "neutral"
            )
            score = float(
                payload.get("confidence")
                or payload.get("score")
                or payload.get("sentiment_score")
                or 0.5
            )
            normalized = str(label).strip().lower()
            if normalized not in ("positive", "neutral", "negative"):
                normalized = "neutral"
            return score, normalized
        except httpx.HTTPError as exc:
            logger.warning("[VALSEA] v1 sentiment failed, trying legacy: %s", exc)

        legacy_url = f"{base}/sentiment"
        legacy_response = await client.post(
            legacy_url,
            headers={**_auth_headers(), "Content-Type": "application/json"},
            json={"text": transcript},
        )
        legacy_response.raise_for_status()
        legacy_payload = legacy_response.json()
        label = legacy_payload.get("label") or legacy_payload.get("sentiment_label") or "neutral"
        score = float(legacy_payload.get("score") or legacy_payload.get("sentiment_score") or 0.5)
        normalized = str(label).strip().lower()
        if normalized not in ("positive", "neutral", "negative"):
            normalized = "neutral"
        return score, normalized
