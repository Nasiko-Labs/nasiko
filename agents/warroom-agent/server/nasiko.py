import json
import logging
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from server.fallback import load_default_company_context
from server.schemas import CompanyContext, CompetitiveSignal
from server import dronahq

logger = logging.getLogger("warroom-nasiko")

AGENT_CARD_PATH = Path(__file__).resolve().parent.parent / "AgentCard.json"
_CACHED_AGENT_CARD: dict[str, Any] | None = None


def get_agent_card() -> dict[str, Any]:
    """Load and return the AgentCard.json metadata."""
    global _CACHED_AGENT_CARD
    if _CACHED_AGENT_CARD is None:
        if not AGENT_CARD_PATH.exists():
            logger.error("AgentCard.json not found at %s", AGENT_CARD_PATH)
            return {"error": "AgentCard.json not found"}
        with open(AGENT_CARD_PATH, "r", encoding="utf-8") as f:
            _CACHED_AGENT_CARD = json.load(f)
    return _CACHED_AGENT_CARD


def jsonrpc_error(req_id: Any, code: int, message: str) -> dict[str, Any]:
    """Create a standard JSON-RPC 2.0 error response."""
    return {
        "jsonrpc": "2.0",
        "id": req_id,
        "error": {
            "code": code,
            "message": message,
        },
    }


def a2a_success_response(req_id: Any, text: str) -> dict[str, Any]:
    """Construct an A2A v1.0 standard SendMessageResponse envelope.

    Shape matches Linux Foundation A2A v1.0 spec verified from Nasiko:
    result.task.status.state = "TASK_STATE_COMPLETED"
    result.task.artifacts[0].parts[0].text
    """
    task_id = f"task-{uuid.uuid4().hex[:12]}"
    context_id = f"ctx-{uuid.uuid4().hex[:12]}"
    artifact_id = f"art-{uuid.uuid4().hex[:12]}"
    timestamp = datetime.now(timezone.utc).isoformat()

    return {
        "jsonrpc": "2.0",
        "id": req_id,
        "result": {
            "task": {
                "id": task_id,
                "contextId": context_id,
                "status": {
                    "state": "TASK_STATE_COMPLETED",
                    "timestamp": timestamp,
                },
                "artifacts": [
                    {
                        "artifactId": artifact_id,
                        "parts": [
                            {"text": text}
                        ],
                    }
                ],
            }
        },
    }


def extract_message_text(params: dict[str, Any] | None) -> str:
    """Extract plain text from an A2A SendMessage or message/send payload."""
    if not isinstance(params, dict):
        return ""

    # 1. Standard A2A 1.0: params.message.parts[].text
    message = params.get("message")
    if isinstance(message, dict):
        parts = message.get("parts")
        if isinstance(parts, list):
            texts = []
            for p in parts:
                if isinstance(p, dict):
                    # Handle both { "text": "..." } and { "content": { "text": "..." } }
                    if "text" in p and isinstance(p["text"], str):
                        texts.append(p["text"])
                    elif "content" in p and isinstance(p["content"], dict) and "text" in p["content"]:
                        texts.append(p["content"]["text"])
            if texts:
                return "\n".join(texts).strip()
        if "content" in message and isinstance(message["content"], str):
            return message["content"].strip()
        if "text" in message and isinstance(message["text"], str):
            return message["text"].strip()

    # 2. Tolerant fallbacks: params.query or params.text
    if "query" in params and isinstance(params["query"], str):
        return params["query"].strip()
    if "text" in params and isinstance(params["text"], str):
        return params["text"].strip()

    return ""


def load_context_signals() -> list[CompetitiveSignal]:
    """Load latest live signals from temporary scan cache if available, else empty."""
    cache_path = Path("/tmp/live_scan_result.json")
    if cache_path.exists():
        try:
            with open(cache_path, "r", encoding="utf-8") as f:
                data = json.load(f)
                raw_signals = data.get("signals", [])
                return [CompetitiveSignal(**s) for s in raw_signals]
        except Exception as exc:
            logger.warning("Failed to load cached signals from /tmp: %s", exc)
    return []


async def dispatch_a2a_request(payload: dict[str, Any], scan_func: Any = None) -> dict[str, Any]:
    """Dispatch an inbound A2A JSON-RPC request to the appropriate WARROOM pipeline.

    Supports:
    - method: "SendMessage" (A2A v1.0 standard)
    - method: "message/send" (A2A v0.3 dialect)
    """
    if not isinstance(payload, dict):
        return jsonrpc_error(None, -32600, "Invalid Request: expected JSON object")

    req_id = payload.get("id")

    # Validate JSON-RPC 2.0
    if payload.get("jsonrpc") != "2.0":
        return jsonrpc_error(req_id, -32600, "Invalid Request: jsonrpc must be '2.0'")

    method = payload.get("method")
    if method not in ("SendMessage", "message/send"):
        return jsonrpc_error(req_id, -32601, f"Method not found: '{method}'. Supported: SendMessage, message/send")

    params = payload.get("params")
    text = extract_message_text(params)
    if not text:
        return jsonrpc_error(req_id, -32600, "Invalid Request: message contains no readable text parts")

    lower_text = text.lower()
    default_company, _ = load_default_company_context()

    try:
        # Route 1: Scan intent
        if any(term in lower_text for term in ["competitive scan", "run scan", "scan competitors", "research competitors"]) or (lower_text.startswith("scan ") and len(lower_text) < 40):
            logger.info("A2A dispatch: routing to competitive scan pipeline")
            if scan_func:
                scan_res = await scan_func(None)
            else:
                from server.main import scan_competitors
                scan_res = await scan_competitors(None)

            # Format human-readable summary
            lines = [
                f"WARROOM Competitive Scan Report for {scan_res.company.name}:",
                f"- Monitored Competitors: {len(scan_res.research)}",
                f"- Signals Identified: {len(scan_res.signals)}",
                "",
            ]
            for s in scan_res.signals:
                lines.append(f"• [{s.competitor}] {s.headline} (Significance: {s.significance})")
                if s.recommended_actions:
                    lines.append(f"  Recommended Action: {s.recommended_actions[0]}")
                if s.source_urls:
                    lines.append(f"  Evidence: {s.source_urls[0]}")

            return a2a_success_response(req_id, "\n".join(lines))

        # Route 2: Simulate response intent
        signals = load_context_signals()
        if any(term in lower_text for term in ["how should", "simulate response", "response simulator", "simulate"]) and signals:
            logger.info("A2A dispatch: routing to response simulator pipeline")
            # Pick the most relevant signal (matching competitor name if specified)
            target_signal = signals[0]
            for s in signals:
                if s.competitor.lower() in lower_text:
                    target_signal = s
                    break

            sim = await dronahq.simulate_response(signal=target_signal, company=default_company)
            lines = [
                f"WARROOM Response Simulation for move: [{target_signal.competitor}] {target_signal.headline}",
                "",
                "Product Investigation:",
                f"- Action: {sim.product.actions[0] if sim.product.actions else 'Review integration'}",
                "",
                "Sales Talk Track:",
                f"- Response: \"{sim.sales.talk_track}\"",
                "",
                "Marketing Angle:",
                f"- Message: {sim.marketing.messaging_angles[0] if sim.marketing.messaging_angles else 'Highlight reliability'}",
                "",
                f"Confidence: {sim.confidence}",
            ]
            return a2a_success_response(req_id, "\n".join(lines))

        # Route 3: Contextual Q&A (Ask WARROOM)
        logger.info("A2A dispatch: routing to Ask WARROOM pipeline")
        ask_res = await dronahq.ask_warroom(
            question=text,
            signals=signals,
            company=default_company,
        )

        lines = [ask_res.answer, ""]
        if ask_res.key_points:
            lines.append("Key Takeaways:")
            for kp in ask_res.key_points[:4]:
                lines.append(f"- {kp}")
            lines.append("")

        if ask_res.evidence.source_urls:
            lines.append("Verified Evidence Sources:")
            for u in ask_res.evidence.source_urls[:3]:
                lines.append(f"- {u}")
            lines.append("")

        lines.append(f"Confidence: {ask_res.confidence}")
        return a2a_success_response(req_id, "\n".join(lines).strip())

    except Exception as exc:
        logger.error("A2A dispatch execution failed: %s", exc, exc_info=True)
        return jsonrpc_error(req_id, -32603, f"Internal processing error: {exc}")
