import asyncio
import json
import os
import time
from contextlib import asynccontextmanager
from pathlib import Path

import httpx
import redis.asyncio as redis
from fastapi import FastAPI, Request, Response
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import HTMLResponse, JSONResponse
from fastapi.staticfiles import StaticFiles

from . import cache, ratelimit, stats
from .queue_worker import queue_worker

REDIS_URL = os.getenv("REDIS_URL", "redis://redis:6379")
KONG_URL = os.getenv("KONG_URL", "http://kong-gateway:8000")

app_state = {}

@asynccontextmanager
async def lifespan(app: FastAPI):
    # Startup
    print("ARIA: Connecting to Redis...")
    app_state["redis"] = redis.from_url(REDIS_URL, decode_responses=False)
    app_state["http_client"] = httpx.AsyncClient()

    # Verify Redis connection
    try:
        await app_state["redis"].ping()
        print("ARIA: Redis connected")
    except Exception as e:
        print(f"ARIA: Redis connection failed: {e}")

    # Pre-load the sentence transformer model
    try:
        cache._get_model()
    except Exception as e:
        print(f"ARIA: Model load warning: {e}")

    # Start background queue worker
    worker_task = asyncio.create_task(
        queue_worker(app_state["redis"], app_state["http_client"])
    )
    app_state["worker_task"] = worker_task

    print("ARIA: Service started successfully")
    yield

    # Shutdown
    print("ARIA: Shutting down...")
    worker_task.cancel()
    try:
        await worker_task
    except asyncio.CancelledError:
        pass
    await app_state["http_client"].aclose()
    await app_state["redis"].close()


app = FastAPI(
    title="ARIA - Adaptive Request Intelligence Architecture",
    lifespan=lifespan,
)

# Mount static files for dashboard CSS/JS
_static_dir = Path(__file__).parent / "static"
if _static_dir.exists():
    app.mount("/static", StaticFiles(directory=str(_static_dir)), name="static")

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


@app.api_route(
    "/agents/{agent_name}/{path:path}",
    methods=["GET", "POST", "PUT", "PATCH", "DELETE"],
)
@app.api_route(
    "/agents/{agent_name}",
    methods=["GET", "POST", "PUT", "PATCH", "DELETE"],
)
async def proxy_request(agent_name: str, request: Request, path: str = ""):
    start_time = time.perf_counter()
    request_body = await request.body()

    # Parse JSON body (or use empty dict for GET)
    try:
        if request_body:
            request_json = json.loads(request_body)
        else:
            request_json = {}
    except json.JSONDecodeError:
        request_json = {}

    query = ""
    if isinstance(request_json, dict):
        query = (
            request_json.get("query", "")
            or request_json.get("message", "")
            or request_json.get("input", "")
            or ""
        )

    # Normalize body bytes for cache key consistency
    if request_body:
        cache_body = request_body
    else:
        cache_body = json.dumps(request_json).encode()

    # 1. Exact Cache Check
    exact_result = await cache.get_exact_cache(
        app_state["redis"], agent_name, cache_body
    )
    if exact_result is not None:
        latency = (time.perf_counter() - start_time) * 1000
        return {
            "source": "exact_cache",
            "latency_ms": round(latency, 2),
            "data": exact_result,
        }

    # 2. Semantic Cache Check
    sem_result = await cache.get_semantic_cache(
        app_state["redis"], agent_name, query
    )
    if sem_result is not None:
        cached_response, similarity = sem_result
        latency = (time.perf_counter() - start_time) * 1000
        return {
            "source": "semantic_cache",
            "latency_ms": round(latency, 2),
            "similarity": round(similarity, 4),
            "data": cached_response,
        }

    stats.stats["misses"] += 1

    # 3. Rate Limiting and Queuing
    is_limited, queue_pos = await ratelimit.is_rate_limited(
        app_state["redis"], agent_name, path, cache_body, request_json
    )
    if is_limited:
        latency = (time.perf_counter() - start_time) * 1000
        if queue_pos == -1:
            return JSONResponse(
                status_code=429,
                content={
                    "source": "rejected",
                    "latency_ms": round(latency, 2),
                    "error": "Rate limit exceeded and queue is full.",
                },
            )
        else:
            return JSONResponse(
                status_code=202,
                content={
                    "source": "queued",
                    "latency_ms": round(latency, 2),
                    "queue_position": queue_pos,
                    "message": "Request queued, will be processed shortly.",
                },
            )

    # 4. Forward to Agent via Kong
    # Kong routes are: /agents/agent-{name} (with agent- prefix, no sub-path)
    # or /agents/{name}/{path} for custom routing
    try:
        if path:
            agent_url = f"{KONG_URL}/agents/{agent_name}/{path}".rstrip("/")
        else:
            agent_url = f"{KONG_URL}/agents/{agent_name}"
        # Also try with "agent-" prefix if the name doesn't start with it
        if not agent_name.startswith("agent-"):
            agent_url_alt = f"{KONG_URL}/agents/agent-{agent_name}"
        else:
            agent_url_alt = None
        method = request.method.upper()

        async def _forward(url):
            if method == "GET":
                r = await app_state["http_client"].get(url, timeout=60.0)
            else:
                r = await app_state["http_client"].request(
                    method, url,
                    content=request_body,
                    headers={"Content-Type": "application/json"},
                    timeout=60.0,
                )
            return r

        # Try primary URL, then fallback with agent- prefix
        resp = None
        try:
            resp = await _forward(agent_url)
            resp.raise_for_status()
        except (httpx.HTTPStatusError, httpx.ConnectError):
            if agent_url_alt:
                try:
                    resp = await _forward(agent_url_alt)
                    resp.raise_for_status()
                except httpx.HTTPStatusError as e2:
                    return JSONResponse(
                        status_code=e2.response.status_code,
                        content={"error": f"Agent error: {e2.response.text}"},
                    )
                except httpx.ConnectError:
                    return JSONResponse(
                        status_code=502,
                        content={"error": f"Cannot reach agent '{agent_name}' via Kong"},
                    )
            else:
                if resp and hasattr(resp, 'status_code'):
                    return JSONResponse(
                        status_code=resp.status_code,
                        content={"error": f"Agent error: {resp.text}"},
                    )
                return JSONResponse(
                    status_code=502,
                    content={"error": f"Cannot reach agent '{agent_name}' via Kong"},
                )

        try:
            agent_data = resp.json()
        except Exception:
            agent_data = {"raw": resp.text}

        # 5. Store in Cache
        await cache.set_cache(
            app_state["redis"], agent_name, cache_body, query, agent_data
        )
        stats.stats["per_agent_requests"][agent_name] += 1
        stats.record_request(agent_name)

        latency = (time.perf_counter() - start_time) * 1000
        return {
            "source": "agent",
            "latency_ms": round(latency, 2),
            "data": agent_data,
        }

    except Exception as e:
        return JSONResponse(
            status_code=500,
            content={"error": f"Internal proxy error: {str(e)}"},
        )


# --- Monitoring Endpoints ---

@app.get("/stats")
async def get_stats_endpoint():
    return stats.get_stats()


@app.post("/demo/start")
async def start_demo():
    """
    Simulate realistic traffic for hackathon demo.
    Generates cache hits, semantic matches, queue pressure, and
    proactive tightening events over ~10 seconds.
    """
    import random

    demo_agents = ["a2a-translator", "a2a-compliance-checker", "a2a-github-agent"]

    # Register all demo agents
    for agent in demo_agents:
        stats.known_agents.add(agent)
        if agent not in stats.stats["per_agent_rate_limit"]:
            stats.stats["per_agent_rate_limit"][agent] = 10.0

    async def run_demo():
        # Phase 1: Normal traffic (0-3s) — cache warming
        for _ in range(8):
            await asyncio.sleep(0.3)
            agent = random.choice(demo_agents)
            stats.stats["misses"] += 1
            stats.stats["per_agent_requests"][agent] += 1
            stats.record_request(agent)

        # Phase 2: Cache hits start flowing (3-6s)
        for _ in range(15):
            await asyncio.sleep(0.2)
            agent = random.choice(demo_agents)
            if random.random() < 0.5:
                stats.stats["exact_hits"] += 1
                stats.stats["per_agent_exact_hits"][agent] += 1
            else:
                stats.stats["semantic_hits"] += 1
                stats.stats["per_agent_semantic_hits"][agent] += 1
            stats.stats["per_agent_requests"][agent] += 1
            stats.record_request(agent)

        # Phase 3: Traffic spike on compliance-checker (6-8s)
        spike_agent = "a2a-compliance-checker"
        stats.stats["per_agent_velocity"][spike_agent] = 3.8
        stats.stats["per_agent_queue_len"][spike_agent] = 12
        stats.stats["queued"] += 12
        for _ in range(10):
            await asyncio.sleep(0.2)
            stats.stats["exact_hits"] += 1
            stats.stats["per_agent_requests"][spike_agent] += 1
            stats.record_request(spike_agent)

        # Phase 4: ARIA intervenes — proactive tightening (8-9s)
        stats.record_proactive_tightening(spike_agent, "traffic accelerating")
        stats.stats["per_agent_velocity"][spike_agent] = 1.2
        stats.stats["per_agent_queue_len"][spike_agent] = 5
        await asyncio.sleep(1.0)

        # Phase 5: System stabilizes (9-10s)
        for agent in demo_agents:
            stats.stats["per_agent_velocity"][agent] = round(random.uniform(-0.5, 0.8), 1)
            stats.stats["per_agent_queue_len"][agent] = random.randint(0, 3)
        # Final cache hits
        for _ in range(8):
            await asyncio.sleep(0.15)
            agent = random.choice(demo_agents)
            stats.stats["semantic_hits"] += 1
            stats.stats["per_agent_requests"][agent] += 1
            stats.record_request(agent)

    asyncio.create_task(run_demo())
    return {"status": "demo_started", "duration_seconds": 10}


@app.post("/config/rate-limit")
async def set_rate_limit(request: Request, agent_name: str = None, limit: float = None):
    # Support both query params and JSON body
    if agent_name and limit:
        pass  # use query params
    else:
        try:
            data = await request.json()
            agent_name = data.get("agent_name", agent_name)
            limit = data.get("limit", limit)
        except Exception:
            pass
    if agent_name and isinstance(limit, (int, float)) and limit > 0:
        await ratelimit.update_rate_limit(agent_name, float(limit))
        return Response(status_code=204)
    return JSONResponse(
        status_code=400,
        content={"error": "Provide agent_name and a positive limit."},
    )


@app.delete("/cache/{agent_name}")
async def clear_cache_endpoint(agent_name: str):
    await cache.clear_agent_cache(app_state["redis"], agent_name)
    return Response(status_code=204)


@app.post("/proxy/{agent_name}/health")
async def test_agent_health(agent_name: str):
    """Test if an agent is reachable via Kong."""
    try:
        agent_url = f"{KONG_URL}/agents/{agent_name}/health"
        resp = await app_state["http_client"].get(agent_url, timeout=10.0)
        return {
            "agent": agent_name,
            "reachable": True,
            "status_code": resp.status_code,
        }
    except Exception as e:
        return {
            "agent": agent_name,
            "reachable": False,
            "error": str(e),
        }


@app.get("/health")
async def health_check():
    redis_ok = False
    try:
        await app_state["redis"].ping()
        redis_ok = True
    except Exception:
        pass
    return {
        "status": "ok" if redis_ok else "degraded",
        "redis": "connected" if redis_ok else "disconnected",
        "service": "aria-request-layer",
    }


@app.get("/dashboard", response_class=HTMLResponse)
async def get_dashboard():
    for p in [
        Path(__file__).parent / "dashboard.html",
        Path("src/dashboard.html"),
        Path("/app/src/dashboard.html"),
    ]:
        if p.exists():
            return HTMLResponse(content=p.read_text(encoding="utf-8"))
    return HTMLResponse(
        content="<h1>Dashboard HTML not found</h1>", status_code=404
    )
