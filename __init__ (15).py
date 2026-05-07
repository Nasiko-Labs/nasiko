"""
MCP Calculator Server v2 — A2A-compatible HTTP agent for the Nasiko platform.

This server:
- Listens on port 5000 (what Nasiko expects)
- Implements the A2A JSON-RPC protocol (tasks/send, agent/info)
- Serves AgentCard at GET / (what Kong checks)
- Does real math: add, subtract, multiply, divide, power, sqrt
- Sends OpenTelemetry traces to Phoenix at port 4318
- Handles all chat messages from the Nasiko web UI

No LLM needed — pure math engine with smart query parsing.
"""

import re
import os
import json
import math
import logging
import time
import uuid
from typing import Any

import uvicorn
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse
from starlette.routing import Route

# ── Logging ────────────────────────────────────────────────────────
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
)
logger = logging.getLogger("mcp-calculator")

# ── Optional: OpenTelemetry tracing ────────────────────────────────
_tracer = None

def _init_tracing():
    """Initialize OpenTelemetry tracing if packages are available."""
    global _tracer
    if os.getenv("TRACING_ENABLED", "true").lower() == "false":
        logger.info("Tracing disabled by TRACING_ENABLED=false")
        return

    # Phoenix 14.x serves OTLP on port 6006 (same as web UI).
    # The orchestrator sets PHOENIX_COLLECTOR_ENDPOINT correctly to :6006.
    endpoint = os.getenv(
        "PHOENIX_COLLECTOR_ENDPOINT",
        os.getenv("OTEL_EXPORTER_OTLP_ENDPOINT", "http://phoenix-observability:6006/v1/traces"),
    )
    # Ensure the endpoint ends with /v1/traces
    if not endpoint.endswith("/v1/traces"):
        endpoint = endpoint.rstrip("/") + "/v1/traces"

    try:
        from opentelemetry import trace
        from opentelemetry.sdk.trace import TracerProvider
        from opentelemetry.sdk.resources import Resource
        from opentelemetry.sdk.trace.export import SimpleSpanProcessor
        from opentelemetry.exporter.otlp.proto.http.trace_exporter import (
            OTLPSpanExporter,
        )

        resource = Resource.create({
            "service.name": "mcp-calculator-server",
            "project.name": "mcp-calculator",
        })
        provider = TracerProvider(resource=resource)
        provider.add_span_processor(
            SimpleSpanProcessor(OTLPSpanExporter(endpoint=endpoint))
        )
        trace.set_tracer_provider(provider)
        _tracer = trace.get_tracer("mcp.calculator")
        logger.info(f"✅ Tracing active → {endpoint}")
    except ImportError:
        logger.warning("⚠️ OTel packages not installed, tracing disabled")
    except Exception as e:
        logger.warning(f"⚠️ Tracing init failed: {e}")


# ── Calculator engine ──────────────────────────────────────────────

def evaluate_math(query: str) -> dict[str, Any]:
    """Parse a natural language math query and return the result.

    Returns a dict with 'answer' (str) and 'operation' (str).
    """
    q = query.lower().strip()
    numbers = [float(x) for x in re.findall(r"-?\d+\.?\d*", q)]

    # Square root (single number)
    if any(word in q for word in ["sqrt", "square root", "root"]):
        if not numbers:
            return {"answer": "Please provide a number for square root.", "operation": "error"}
        n = numbers[0]
        if n < 0:
            return {"answer": f"Cannot compute square root of negative number {n}.", "operation": "error"}
        result = math.sqrt(n)
        return {"answer": f"√{n} = {result}", "operation": "sqrt"}

    # Need at least 2 numbers for binary operations
    if len(numbers) < 2:
        if len(numbers) == 1:
            return {"answer": f"The number is {numbers[0]}. Try asking me to add, subtract, multiply, or divide two numbers!", "operation": "identity"}
        return {
            "answer": "I'm a calculator! Try: 'add 40 and 2', 'multiply 6 by 7', 'divide 100 by 4', 'sqrt 144'",
            "operation": "help",
        }

    a, b = numbers[0], numbers[1]

    if any(word in q for word in ["add", "plus", "sum", "+", "total"]):
        result = a + b
        return {"answer": f"{a} + {b} = {result}", "operation": "add"}

    elif any(word in q for word in ["subtract", "minus", "difference", "-", "take away"]):
        result = a - b
        return {"answer": f"{a} - {b} = {result}", "operation": "subtract"}

    elif any(word in q for word in ["multiply", "times", "product", "*", "x"]):
        result = a * b
        return {"answer": f"{a} × {b} = {result}", "operation": "multiply"}

    elif any(word in q for word in ["divide", "divided", "quotient", "/", "over"]):
        if b == 0:
            return {"answer": "Error: Cannot divide by zero!", "operation": "error"}
        result = a / b
        return {"answer": f"{a} ÷ {b} = {result}", "operation": "divide"}

    elif any(word in q for word in ["power", "exponent", "^", "**", "raised"]):
        result = a ** b
        return {"answer": f"{a} ^ {b} = {result}", "operation": "power"}

    elif any(word in q for word in ["modulo", "mod", "remainder", "%"]):
        if b == 0:
            return {"answer": "Error: Cannot compute modulo with zero!", "operation": "error"}
        result = a % b
        return {"answer": f"{a} mod {b} = {result}", "operation": "modulo"}

    else:
        # Default: try to be helpful
        result = a + b
        return {"answer": f"I'll add them: {a} + {b} = {result}. Try 'multiply {a} and {b}' for other operations!", "operation": "add"}


# ── A2A JSON-RPC handlers ─────────────────────────────────────────

def _extract_user_text(params: dict) -> str:
    """Extract the user's text message from A2A JSONRPC params."""
    message = params.get("message", {})
    parts = message.get("parts", [])
    for part in parts:
        if isinstance(part, dict):
            # Platform uses "kind": "text", some clients use "type": "text"
            if part.get("kind") == "text" or part.get("type") == "text" or "text" in part:
                return part.get("text", "")
    # Fallback: check for direct text
    if isinstance(message, str):
        return message
    return str(params)


async def handle_jsonrpc(request: Request) -> JSONResponse:
    """Handle A2A JSON-RPC requests from the Nasiko platform."""
    try:
        body = await request.json()
    except Exception:
        return JSONResponse({"error": "Invalid JSON"}, status_code=400)

    method = body.get("method", "")
    req_id = body.get("id", 1)
    params = body.get("params", {})

    logger.info(f"JSONRPC method={method}")

    # ── agent/info ────────────────────────────────────────────
    if method == "agent/info":
        return JSONResponse({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": _get_agent_card_dict(),
        })

    # ── message/send OR tasks/send ────────────────────────────
    # The Nasiko web UI sends "message/send" (A2A v0.2.9 protocol)
    # Some A2A clients may send "tasks/send" (older protocol)
    elif method in ("message/send", "tasks/send"):
        user_text = _extract_user_text(params)
        task_id = params.get("id", str(uuid.uuid4()))

        # Trace the tool call if tracing is enabled
        span = None
        if _tracer is not None:
            try:
                from opentelemetry.trace import StatusCode
                span = _tracer.start_span(f"mcp.calculator.{task_id}")
                span.set_attribute("mcp.tool.query", user_text)
                span.set_attribute("mcp.server.name", "mcp-calculator-server")
                span.set_attribute("mcp.transport", "http-jsonrpc")
            except Exception:
                pass

        # Calculate
        calc_result = evaluate_math(user_text)
        answer = calc_result["answer"]
        operation = calc_result["operation"]

        logger.info(f"Query: '{user_text}' → {operation}: {answer}")

        # Record in span
        if span is not None:
            try:
                from opentelemetry.trace import StatusCode
                span.set_attribute("mcp.tool.operation", operation)
                span.set_attribute("mcp.tool.result", answer)
                span.set_status(StatusCode.OK)
                span.end()
            except Exception:
                pass

        return JSONResponse({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "kind": "message",
                "role": "agent",
                "messageId": str(uuid.uuid4()),
                "parts": [{"kind": "text", "text": answer}],
                "metadata": {
                    "taskId": task_id,
                    "operation": operation,
                },
            },
        })

    # ── tasks/get ─────────────────────────────────────────────
    elif method == "tasks/get":
        return JSONResponse({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "id": params.get("id", "task-1"),
                "status": {"state": "completed"},
            },
        })

    # ── tasks/cancel ──────────────────────────────────────────
    elif method == "tasks/cancel":
        return JSONResponse({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "id": params.get("id", "task-1"),
                "status": {"state": "canceled"},
            },
        })

    # ── unknown method ────────────────────────────────────────
    else:
        return JSONResponse({
            "jsonrpc": "2.0",
            "id": req_id,
            "error": {"code": -32601, "message": f"Method not found: {method}"},
        })


# ── AgentCard ──────────────────────────────────────────────────────

def _get_agent_card_dict() -> dict:
    """Return the agent card as a Python dict."""
    return {
        "name": "MCP Calculator v2",
        "description": "A math calculator agent that performs arithmetic operations. Supports: add, subtract, multiply, divide, power, square root, and modulo. Built for the Nasiko MCP Hackathon.",
        "version": "1.0.0",
        "url": "http://localhost:5000/",
        "capabilities": {
            "streaming": False,
            "pushNotifications": False,
            "stateTransitionHistory": False,
        },
        "defaultInputModes": ["text/plain", "application/json"],
        "defaultOutputModes": ["text/plain", "application/json"],
        "skills": [
            {
                "id": "add",
                "name": "Addition",
                "description": "Add two numbers together. Example: 'add 40 and 2'",
                "tags": ["math", "calculator", "addition"],
                "examples": ["add 40 and 2", "what is 100 + 200"],
            },
            {
                "id": "subtract",
                "name": "Subtraction",
                "description": "Subtract one number from another. Example: 'subtract 10 from 50'",
                "tags": ["math", "calculator", "subtraction"],
                "examples": ["subtract 10 from 50", "what is 100 - 25"],
            },
            {
                "id": "multiply",
                "name": "Multiplication",
                "description": "Multiply two numbers. Example: 'multiply 6 by 7'",
                "tags": ["math", "calculator", "multiplication"],
                "examples": ["multiply 6 by 7", "what is 12 times 5"],
            },
            {
                "id": "divide",
                "name": "Division",
                "description": "Divide one number by another. Example: 'divide 100 by 4'",
                "tags": ["math", "calculator", "division"],
                "examples": ["divide 100 by 4", "what is 50 / 10"],
            },
            {
                "id": "power",
                "name": "Exponentiation",
                "description": "Raise a number to a power. Example: '2 power 8'",
                "tags": ["math", "calculator", "power"],
                "examples": ["2 power 8", "3 raised to 4"],
            },
            {
                "id": "sqrt",
                "name": "Square Root",
                "description": "Calculate the square root of a number. Example: 'sqrt 144'",
                "tags": ["math", "calculator", "sqrt"],
                "examples": ["sqrt 144", "square root of 256"],
            },
        ],
    }


async def agent_card_get(request: Request) -> JSONResponse:
    """Serve AgentCard at GET / — what Kong and the platform check."""
    return JSONResponse(_get_agent_card_dict())


async def health(request: Request) -> JSONResponse:
    """Health check endpoint."""
    return JSONResponse({"status": "ok", "service": "mcp-calculator-server"})


# ── Starlette app ──────────────────────────────────────────────────

app = Starlette(routes=[
    Route("/", agent_card_get, methods=["GET"]),
    Route("/", handle_jsonrpc, methods=["POST"]),
    Route("/health", health, methods=["GET"]),
])


# ── Entrypoint ─────────────────────────────────────────────────────

if __name__ == "__main__":
    _init_tracing()
    logger.info("Starting MCP Calculator Server on 0.0.0.0:5000")
    uvicorn.run(app, host="0.0.0.0", port=5000, log_level="info")
