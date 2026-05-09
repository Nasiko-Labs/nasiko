"""
Hackathon demo FastAPI app for the Nasiko request-management layer.

This file is intentionally small and easy to read. It shows how the standalone
request_manager module can sit in front of agent execution without wiring into
the full Nasiko platform yet.

Request flow for the demo:
1. The client sends JSON to /process-request.
2. The app hands the request to RequestManager.
3. RequestManager checks cache first.
4. If there is no cache hit, RequestManager applies rate limiting.
5. Overflow traffic is placed into an asyncio queue.
6. A background worker processes the queue and calls a mock agent.
7. Metrics are updated on every path.

TODO: Replace the mock agent execution with a real Nasiko router/agent call.
TODO: Connect this app to the actual Nasiko gateway or router service later.
"""

from __future__ import annotations

import logging
from contextlib import asynccontextmanager
from time import perf_counter

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel, Field

from request_manager import RequestManager


logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
)
logger = logging.getLogger(__name__)


class ProcessRequestBody(BaseModel):
    """Input payload for the demo endpoint.

    The shape stays intentionally tiny so the hackathon demo is easy to test.
    """

    agent_name: str = Field(..., description="Target agent name, for example translator-agent")
    query: str = Field(..., description="User request text")


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Create the middleware manager once and keep it for the app lifetime.

    This is the simplest way to make the queue worker available for all
    incoming requests without introducing more infrastructure.
    """

    # A longer mock delay makes the first uncached request visibly slower.
    # The queue is kept small so overload behavior is easy to demonstrate.
    agent_limits = {
        # These per-agent limits are intentionally tiny for the hackathon demo.
        # They make overload behavior visible within just a few requests.
        "translator-agent": {"max_requests": 3, "window_seconds": 10},
        "coding-agent": {"max_requests": 2, "window_seconds": 10},
        "summarizer-agent": {"max_requests": 3, "window_seconds": 10},
    }
    app.state.request_manager = RequestManager(
        cache_ttl_seconds=60,
        rate_limit_max_requests=3,
        rate_limit_window_seconds=10,
        queue_maxsize=2,
        mock_delay_seconds=2.0,
        agent_limits=agent_limits,
    )
    await app.state.request_manager.start()
    logger.info("Request manager started for demo app")

    try:
        yield
    finally:
        await app.state.request_manager.stop()
        logger.info("Request manager stopped for demo app")


app = FastAPI(
    title="Nasiko Request Manager Demo",
    description="A simple FastAPI demo showing cache, rate limiting, queueing, and metrics.",
    version="0.1.0",
    lifespan=lifespan,
)


@app.post("/process-request")
async def process_request(body: ProcessRequestBody):
    """Send a request through the middleware skeleton and return the result.

    This endpoint is the main demo path. It deliberately stays thin and simply
    forwards the request to RequestManager so the middleware behavior is visible.
    """

    request_manager: RequestManager = app.state.request_manager
    started_at = perf_counter()

    try:
        # The middleware handles cache lookup, rate limiting, queueing,
        # and the mock agent call. The app only passes data through.
        result = await request_manager.handle_request(
            agent_id=body.agent_name,
            payload={"query": body.query},
        )

        elapsed_ms = (perf_counter() - started_at) * 1000
        source = result.get("source", "unknown")

        # Add one small, explicit log line so the demo output is easy to follow.
        logger.info(
            "demo_request_complete agent=%s source=%s elapsed_ms=%.2f",
            body.agent_name,
            source,
            elapsed_ms,
        )

        # The result includes the mock agent payload plus the source of truth.
        return {
            "agent_name": body.agent_name,
            "query": body.query,
            "source": source,
            "elapsed_ms": round(elapsed_ms, 2),
            "result": result["response"],
        }

    except Exception as exc:
        logger.exception("process_request_failed agent=%s", body.agent_name)
        raise HTTPException(status_code=500, detail=str(exc)) from exc


@app.get("/stats")
async def stats():
    """Return all demo metrics as JSON.

    This is intentionally simple: one endpoint exposes the in-memory counters
    so you can watch cache hits, queueing, and request timing during the demo.
    """

    request_manager: RequestManager = app.state.request_manager
    metrics = request_manager.get_metrics()
    logger.info("stats_requested total_requests=%s", metrics["total_requests"])
    return metrics


@app.get("/health")
async def health():
    """Very small health check used by the demo and by future integrations."""

    request_manager: RequestManager = app.state.request_manager
    return {
        "status": "ok",
        "queue_depth": request_manager.get_metrics()["queue_depth"],
    }


if __name__ == "__main__":
    import uvicorn

    # Use --reload only for local demo work.
    uvicorn.run("hackathon_demo_app:app", host="0.0.0.0", port=8000, reload=True)