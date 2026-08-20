import time
import asyncio
from contextlib import asynccontextmanager

import redis.asyncio as aioredis
from fastapi import FastAPI, HTTPException, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse

from gateway.config import REDIS_URL
from gateway.cache.cache_manager import cache_manager
from gateway.rate_limiter.token_bucket import rate_limiter
from gateway.routing.proxy import handle_request, init_fleet
from gateway.admin.router import router as admin_router
from gateway.models.schemas import AgentRequest, AgentResponse


# ─── Lifespan ─────────────────────────────────────────────────────────────────

@asynccontextmanager
async def lifespan(app: FastAPI):
    # Startup
    print("[Gateway] Starting up...")
    redis_client = await aioredis.from_url(REDIS_URL, decode_responses=True)
    await cache_manager.connect()
    await rate_limiter.connect(redis_client)
    init_fleet()
    print("[Gateway] Ready ✓")

    yield

    # Shutdown
    print("[Gateway] Shutting down...")
    if redis_client:
        await redis_client.aclose()


# ─── App ──────────────────────────────────────────────────────────────────────

app = FastAPI(
    title="Resilient Agent Request Layer",
    description=(
        "A unified request management layer providing intelligent caching, "
        "adaptive per-agent rate limiting, and request queuing for AI agent fleets."
    ),
    version="1.0.0",
    lifespan=lifespan,
)

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

# Mount admin router
app.include_router(admin_router)


# ─── Main Invoke Endpoint ─────────────────────────────────────────────────────

@app.post("/invoke", response_model=AgentResponse)
async def invoke_agent(body: AgentRequest, request: Request):
    """
    Main gateway endpoint. Routes requests through cache → rate limiter → queue → agent.
    """
    start = time.perf_counter()
    headers = dict(request.headers)

    try:
        result, source = await handle_request(
            agent_id=body.agent_id,
            payload=body.payload,
            headers=headers,
            bypass_cache=body.bypass_cache,
            priority=body.priority,
        )
    except asyncio.TimeoutError as e:
        raise HTTPException(status_code=503, detail=f"Request timed out in queue: {e}")
    except RuntimeError as e:
        msg = str(e)
        if "Queue full" in msg:
            raise HTTPException(
                status_code=429,
                detail=msg,
                headers={"Retry-After": "5"},
            )
        raise HTTPException(status_code=502, detail=msg)
    except Exception as e:
        raise HTTPException(status_code=502, detail=f"Agent error: {e}")

    latency_ms = (time.perf_counter() - start) * 1000

    return AgentResponse(
        agent_id=body.agent_id,
        source=source,
        latency_ms=round(latency_ms, 2),
        data=result,
    )


# ─── Health Check ─────────────────────────────────────────────────────────────

@app.get("/health")
async def health():
    return {"status": "ok", "timestamp": time.time()}


# ─── 404 fallback ─────────────────────────────────────────────────────────────

@app.exception_handler(404)
async def not_found(request: Request, exc):
    return JSONResponse(status_code=404, content={"detail": "Not found"})
