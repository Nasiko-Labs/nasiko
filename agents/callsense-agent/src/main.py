"""
CallSense Nasiko agent — voice middleware with VALSEA STT + sentiment.

TTS (audio_base64) is produced by the CallSense Next.js backend (ElevenLabs).
VALSEA handles speech-to-text and sentiment only.
"""

from __future__ import annotations

import logging
import os
from typing import Any, Optional

import httpx
from fastapi import FastAPI, HTTPException
from pydantic import BaseModel, Field

from src.valsea_client import analyze_sentiment, transcribe_audio_base64

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("callsense-agent")

DEFAULT_BACKEND_URL = "https://cursor-buildathon-pi.vercel.app"
BACKEND_URL = os.getenv("CALLSENSE_BACKEND_URL", DEFAULT_BACKEND_URL).rstrip("/")
PROCESS_PATH = os.getenv("CALLSENSE_PROCESS_PATH", "/api/calls/process")
PROCESS_URL = f"{BACKEND_URL}{PROCESS_PATH}"
HTTP_TIMEOUT = float(os.getenv("CALLSENSE_HTTP_TIMEOUT", "60"))
VALSEA_TIMEOUT = float(os.getenv("VALSEA_HTTP_TIMEOUT", "90"))

app = FastAPI(
    title="CallSense Voice Processing Agent",
    version="1.0.0",
    description="Nasiko agent: VALSEA transcription → CallSense backend → ElevenLabs TTS",
)


class ProcessAudioRequest(BaseModel):
    business_id: str = Field(..., min_length=1)
    phone_number: str = Field(..., min_length=1)
    audio_base64: Optional[str] = None
    transcript: Optional[str] = None
    text: Optional[str] = None
    sentiment_score: Optional[float] = Field(default=None, ge=0.0, le=1.0)
    sentiment_label: Optional[str] = None
    language: Optional[str] = None


def normalize_sentiment_label(label: str) -> str:
    normalized = label.strip().lower()
    if normalized in ("positive", "neutral", "negative"):
        return normalized
    return "neutral"


async def resolve_transcript(payload: ProcessAudioRequest) -> str:
    if payload.transcript and payload.transcript.strip():
        transcript = payload.transcript.strip()
        logger.info("[VALSEA] using provided transcript (%d chars)", len(transcript))
        return transcript

    if payload.text and payload.text.strip():
        transcript = payload.text.strip()
        logger.info("[VALSEA] using provided text (%d chars)", len(transcript))
        return transcript

    if payload.audio_base64:
        logger.info("[VALSEA] transcribing audio for %s", payload.phone_number)
        return await transcribe_audio_base64(
            payload.audio_base64,
            language=payload.language,
            timeout=VALSEA_TIMEOUT,
        )

    raise HTTPException(
        status_code=400,
        detail={
            "error": True,
            "message": "Provide transcript, text, or audio_base64",
        },
    )


async def resolve_sentiment(
    transcript: str,
    payload: ProcessAudioRequest,
) -> tuple[float, str]:
    if payload.sentiment_score is not None and payload.sentiment_label:
        return payload.sentiment_score, normalize_sentiment_label(payload.sentiment_label)

    try:
        score, label = await analyze_sentiment(transcript, timeout=VALSEA_TIMEOUT)
        logger.info("[VALSEA] sentiment=%s score=%.2f", label, score)
        return score, label
    except Exception as exc:
        logger.warning("[VALSEA] sentiment fallback: %s", exc)
        return 0.8, "positive"


@app.get("/health")
async def health() -> dict[str, str]:
    has_key = bool(os.getenv("VALSEA_API_KEY") or os.getenv("VALSA_API_KEY"))
    return {
        "status": "healthy",
        "service": "callsense-agent",
        "valsea_configured": str(has_key).lower(),
    }


@app.post("/process-audio")
async def process_audio(payload: ProcessAudioRequest) -> dict[str, Any]:
    try:
        transcript = await resolve_transcript(payload)
        sentiment_score, sentiment_label = await resolve_sentiment(transcript, payload)

        backend_body = {
            "business_id": payload.business_id,
            "phone_number": payload.phone_number,
            "transcript": transcript,
            "sentiment_score": sentiment_score,
            "sentiment_label": sentiment_label,
        }

        logger.info(
            "Forwarding to CallSense: %s business_id=%s phone=%s",
            PROCESS_URL,
            payload.business_id,
            payload.phone_number,
        )

        async with httpx.AsyncClient(timeout=HTTP_TIMEOUT) as client:
            response = await client.post(
                PROCESS_URL,
                json=backend_body,
                headers={"Content-Type": "application/json"},
            )

        if response.is_error:
            logger.error(
                "CallSense backend error: status=%s body=%s",
                response.status_code,
                response.text,
            )
            raise HTTPException(
                status_code=response.status_code,
                detail={
                    "error": True,
                    "message": "CallSense backend request failed",
                    "backend_status": response.status_code,
                    "backend_body": response.text,
                },
            )

        try:
            result = response.json()
        except ValueError as exc:
            raise HTTPException(
                status_code=502,
                detail={
                    "error": True,
                    "message": f"Backend returned non-JSON response: {exc}",
                },
            ) from exc

        logger.info(
            "Pipeline ok: call_id=%s transcript_len=%d audio_base64=%s",
            result.get("call_id"),
            len(transcript),
            bool(result.get("audio_base64")),
        )
        return result

    except HTTPException:
        raise
    except ValueError as exc:
        logger.exception("VALSEA configuration error")
        raise HTTPException(
            status_code=500,
            detail={"error": True, "message": str(exc)},
        ) from exc
    except httpx.TimeoutException as exc:
        raise HTTPException(
            status_code=504,
            detail={"error": True, "message": f"Request timeout: {exc}"},
        ) from exc
    except httpx.HTTPStatusError as exc:
        raise HTTPException(
            status_code=502,
            detail={
                "error": True,
                "message": f"VALSEA API error: {exc.response.status_code}",
                "body": exc.response.text,
            },
        ) from exc
    except httpx.RequestError as exc:
        raise HTTPException(
            status_code=502,
            detail={"error": True, "message": f"Network error: {exc}"},
        ) from exc
    except Exception as exc:
        logger.exception("Unexpected error in /process-audio")
        raise HTTPException(
            status_code=500,
            detail={"error": True, "message": str(exc)},
        ) from exc
