"""Smart LLM Router entrypoint.

Run as a package module so relative imports resolve:
  python -m src --host 0.0.0.0 --port 8000
  smart-llm-router   # after pip install
"""

from __future__ import annotations

import logging
import os
from pathlib import Path

from dotenv import load_dotenv

from .telemetry import init_telemetry

load_dotenv(override=True)
logging.basicConfig(level=logging.INFO)
init_telemetry(service_name=os.environ.get("OTEL_SERVICE_NAME", "smart-llm-router-agent"))

import asyncio
import json

import click
import uvicorn
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.routes import create_agent_card_routes, create_jsonrpc_routes
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import AgentCapabilities, AgentCard, AgentInterface, AgentSkill
from starlette.applications import Starlette
from starlette.middleware.cors import CORSMiddleware
from starlette.requests import Request
from starlette.responses import FileResponse, JSONResponse, StreamingResponse
from starlette.routing import Mount, Route
from starlette.staticfiles import StaticFiles

from .agent import SmartLlmRouterAgent
from .agent_executor import SmartLlmRouterAgentExecutor
from .router_client import RouterSuccess, router_base_url
from .turn_bus import BUS, publish_event_dict

logger = logging.getLogger(__name__)
STATIC_DIR = Path(__file__).resolve().parent / "static"

CORS_ORIGINS = [
    "http://localhost:4000",
    "http://127.0.0.1:4000",
    "http://localhost:3000",
    "http://127.0.0.1:3000",
    "http://localhost:8080",
    "http://127.0.0.1:8080",
]


def _dashboard_token() -> str:
    """Shared secret for /api/stats|/api/events|/api/ingest|/api/turn when set."""
    return (os.environ.get("ANALYTICS_TOKEN") or os.environ.get("DASHBOARD_TOKEN") or "").strip()


def _require_dashboard_auth(request: Request) -> JSONResponse | None:
    """When ANALYTICS_TOKEN is set, require matching Bearer / x-analytics-token / ?token=."""
    expected = _dashboard_token()
    if not expected:
        return None
    auth = (request.headers.get("authorization") or "").strip()
    header = (request.headers.get("x-analytics-token") or "").strip()
    query = (request.query_params.get("token") or "").strip()
    bearer = auth[7:].strip() if auth.lower().startswith("bearer ") else ""
    if header == expected or bearer == expected or query == expected:
        return None
    return JSONResponse({"error": "unauthorized"}, status_code=401)


async def ui_index(_request: Request) -> FileResponse:
    """Analytics dashboard — live stream of Nasiko console turns (SSE)."""
    return FileResponse(STATIC_DIR / "index.html")


async def api_stats(request: Request) -> JSONResponse:
    if denied := _require_dashboard_auth(request):
        return denied
    return JSONResponse(BUS.snapshot())


async def api_ingest(request: Request) -> JSONResponse:
    """Receive analytics from a deployed agent container (async fan-in)."""
    if denied := _require_dashboard_auth(request):
        return denied
    try:
        body = await request.json()
    except Exception:
        return JSONResponse({"error": "invalid JSON"}, status_code=400)
    if not isinstance(body, dict):
        return JSONResponse({"error": "expected object"}, status_code=400)
    event = publish_event_dict(body)
    if event is None:
        return JSONResponse({"error": "invalid turn payload"}, status_code=400)
    return JSONResponse({"ok": True, "id": event.id})


async def api_events(request: Request):
    """Server-Sent Events: every console/UI turn after smart routing completes."""
    if denied := _require_dashboard_auth(request):
        return denied
    queue = BUS.subscribe()

    async def gen():
        try:
            # Hello so the client knows the stream is live.
            yield "event: hello\ndata: {\"ok\":true}\n\n"
            while True:
                try:
                    item = await asyncio.wait_for(queue.get(), timeout=20.0)
                except asyncio.TimeoutError:
                    yield ": keepalive\n\n"
                    continue
                if item is None:
                    break
                payload = json.dumps(item.to_dict(), ensure_ascii=False)
                yield f"event: turn\ndata: {payload}\n\n"
        finally:
            BUS.unsubscribe(queue)

    return StreamingResponse(
        gen(),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-cache",
            "Connection": "keep-alive",
            "X-Accel-Buffering": "no",
        },
    )


async def api_turn(request: Request) -> JSONResponse:
    if denied := _require_dashboard_auth(request):
        return denied
    try:
        body = await request.json()
    except Exception:
        return JSONResponse({"error": "invalid JSON"}, status_code=400)
    if not isinstance(body, dict):
        return JSONResponse({"error": "JSON object required"}, status_code=400)
    prompt = str(body.get("prompt") or "").strip()
    session_id = str(body.get("session_id") or "").strip() or None
    if not prompt:
        return JSONResponse({"error": "missing prompt"}, status_code=400)

    agent: SmartLlmRouterAgent = request.app.state.chat_agent
    result = await agent.run_turn(prompt, session_id=session_id, source="ui")
    if not result.ok or not isinstance(result.router, RouterSuccess):
        return JSONResponse(
            {
                "error": result.text.strip(),
                "ok": False,
            },
            status_code=502 if not result.ok else 200,
        )

    r = result.router
    payload = {
        "ok": True,
        "reply": result.text,  # chat-only; analytics via SSE / turn bus
        "requested_tier": result.requested_tier,
        "classification": {
            "task_type": result.classification.task_type,
            "complexity": result.classification.complexity,
            "reasoning": result.classification.reasoning,
            "source": result.classification.source,
        },
        "router": {
            "provider": r.provider,
            "model": r.model,
            "content": r.content,
            "tier_used": r.tier_used,
            "cache_hit": r.cache_hit,
            "latency_ms": r.latency_ms,
            "usage": r.usage,
        },
        "savings": None
        if result.savings is None
        else {
            "baseline_usd": result.savings.baseline_usd,
            "actual_usd": result.savings.actual_usd,
            "saved_usd": result.savings.saved_usd,
            "pct_saved": result.savings.pct_saved,
        },
        "totals": None
        if result.totals is None
        else {
            "running_saved_usd": result.totals.running_saved_usd,
            "running_calls": result.totals.running_calls,
            "running_cache_hits": result.totals.running_cache_hits,
        },
    }
    return JSONResponse(payload)


@click.command()
@click.option("--host", default="0.0.0.0")
@click.option("--port", default=8000)
def main(host: str, port: int) -> None:
    """Starts the Smart LLM Router agent server."""
    skill = AgentSkill(
        id="smart-route",
        name="Smart LLM routing",
        description=(
            "Classify a prompt, pick the cheapest viable Nasiko tier, call /v1/route, "
            "and report cost saved vs always-GPT-4o."
        ),
        tags=["routing", "cost", "tier", "finops"],
        examples=[
            "hi",
            "Prove the Pythagorean theorem.",
            "Translate 'good morning' to French",
        ],
    )
    agent_url = os.getenv("HOST_OVERRIDE", f"http://{host}:{port}/")
    agent_card = AgentCard(
        name="Smart LLM Router",
        description=(
            "Classifies each user prompt, requests cheap/balanced/premium from Nasiko "
            "POST /v1/route, and shows a running cost-saved-vs-GPT-4o counter."
        ),
        supported_interfaces=[
            AgentInterface(protocol_binding="JSONRPC", url=agent_url),
        ],
        version="0.1.0",
        default_input_modes=SmartLlmRouterAgent.SUPPORTED_CONTENT_TYPES,
        default_output_modes=SmartLlmRouterAgent.SUPPORTED_CONTENT_TYPES,
        capabilities=AgentCapabilities(streaming=True),
        skills=[skill],
    )
    request_handler = DefaultRequestHandler(
        agent_executor=SmartLlmRouterAgentExecutor(),
        task_store=InMemoryTaskStore(),
        agent_card=agent_card,
    )

    # A2A JSON-RPC + AgentCard own `/` (Nasiko console).
    # /ui + /api/events = async analytics dashboard for those console turns.
    routes = [
        Route("/ui/", endpoint=ui_index, methods=["GET"]),
        Route("/api/turn", endpoint=api_turn, methods=["POST"]),
        Route("/api/events", endpoint=api_events, methods=["GET"]),
        Route("/api/stats", endpoint=api_stats, methods=["GET"]),
        Route("/api/ingest", endpoint=api_ingest, methods=["POST"]),
        Mount("/ui/assets", app=StaticFiles(directory=str(STATIC_DIR)), name="assets"),
    ]
    routes.extend(create_agent_card_routes(agent_card))
    routes.extend(create_jsonrpc_routes(request_handler, rpc_url="/"))

    app = Starlette(routes=routes)
    app.state.chat_agent = SmartLlmRouterAgent()
    app.add_middleware(
        CORSMiddleware,
        allow_origins=CORS_ORIGINS + [f"http://{host}:{port}", "http://127.0.0.1:8000"],
        allow_credentials=True,
        allow_methods=["*"],
        allow_headers=["*"],
    )
    logger.info(
        "Smart LLM Router on %s:%s (ROUTER_BASE_URL=%s); A2A=/ analytics=/ui/ SSE=/api/events",
        host,
        port,
        router_base_url(),
    )
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()
