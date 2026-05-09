"""
FastAPI application layer for the resilient AI request orchestrator.

This module owns HTTP concerns and composes the middleware pipeline:
validation, cache lookup, cache-miss coalescing, per-agent rate admission,
queue buffering, agent execution, and observability updates. Infrastructure
details stay in their own modules so Redis, distributed queues, real agents,
or external monitoring can be introduced later without rewriting the route.
"""

import asyncio
import logging
import time
from contextlib import asynccontextmanager

from fastapi import FastAPI, HTTPException, Request
from fastapi.responses import JSONResponse
from fastapi.middleware.cors import CORSMiddleware

# Application components are intentionally module-level singletons for a
# lightweight hackathon deployment. Future production wiring can move these
# behind dependency injection or a lifespan-created service container.
from models import RequestModel, ResponseModel, StatusModel
from stats import stats_collector
from cache import cache_manager, generate_cache_key, cache_key_lock
from limiter import rate_limiter
from queue_manager import QueuedRequest, queue_manager
from agents import FakeAgent, agent_registry


logger = logging.getLogger(__name__)


REQUEST_RESPONSE_DOCS = {
    202: {
        "description": "Request was accepted into the overload queue.",
        "content": {
            "application/json": {
                "example": {
                    "status": "queued",
                    "request_id": "2bbda6dd-8a30-4c53-9505-cd2d5e6fb2b3",
                    "agent": "coder",
                    "queue_position": 1,
                    "estimated_wait_time": 0.75,
                    "queue_size": 1,
                    "message": "Request accepted into the resilience queue.",
                    "agent_limit_info": {
                        "agent": "coder",
                        "limit": 3,
                        "window_seconds": 10,
                        "requests_in_window": 3,
                        "remaining_requests": 0,
                        "retry_after": 7.5,
                        "allowed": False,
                    },
                }
            }
        },
    },
    503: {
        "description": "Request could not be queued because the queue is full.",
        "content": {
            "application/json": {
                "example": {
                    "status": "queue_full",
                    "message": "Queue is full; retry later.",
                    "queue_size": 100,
                    "max_queue_size": 100,
                    "estimated_wait_time": 75.0,
                    "agent_limit_info": {
                        "agent": "coder",
                        "remaining_requests": 0,
                        "allowed": False,
                    },
                }
            }
        },
    },
}


# ============================================================================
# REQUEST PROCESSING HELPERS
# ============================================================================

def _normalize_request(req: RequestModel) -> tuple[str, str]:
    """Validate input and normalize agent/query values for stable downstream keys."""
    if not req.agent or not req.agent.strip():
        raise HTTPException(status_code=400, detail="Agent name cannot be empty")

    if not req.query or not req.query.strip():
        raise HTTPException(status_code=400, detail="Query cannot be empty")

    return req.agent.strip().lower(), req.query.strip()


def _get_agent_or_404(agent_name: str) -> FakeAgent:
    """
    Resolve the requested agent or raise an HTTP-native not-found response.

    Keeping registry error translation here keeps the route focused on the
    orchestration pipeline rather than lookup mechanics.
    """
    try:
        return agent_registry.get_agent(agent_name)
    except KeyError as exc:
        message = str(exc.args[0]) if exc.args else str(exc)
        raise HTTPException(status_code=404, detail=message) from exc


def _build_cached_response(agent_name: str, cached_response: str) -> ResponseModel:
    """Build a response for an already-cached value without touching agent capacity."""
    start_time = time.perf_counter()
    processing_time = time.perf_counter() - start_time

    return ResponseModel(
        agent=agent_name,
        response=cached_response,
        cached=True,
        processing_time=processing_time,
    )


async def _process_agent_cache_miss(
    agent_name: str,
    query: str,
    cache_key: str,
    agent: FakeAgent | None = None,
) -> ResponseModel:
    """
    Run an agent for a confirmed cache miss, then persist and observe the result.

    Callers are responsible for performing admission control and cache-miss
    coalescing before reaching this function.
    """
    agent = agent or agent_registry.get_agent(agent_name)
    start_time = time.perf_counter()

    response_text = await agent.process(query)
    processing_time = time.perf_counter() - start_time

    cache_manager.set(cache_key, response_text)
    stats_collector.record_processing_complete(processing_time, agent_name)

    return ResponseModel(
        agent=agent_name,
        response=response_text,
        cached=False,
        processing_time=processing_time,
    )


async def _handle_cache_miss_with_lock(
    agent_name: str,
    query: str,
    cache_key: str,
    agent: FakeAgent | None = None,
) -> ResponseModel | JSONResponse:
    """
    Coalesce duplicate cache misses for the same agent/query pair.

    The first request that misses owns the work. Concurrent identical requests
    wait on the per-key lock, re-check cache, and reuse the populated response.
    """
    async with cache_key_lock(cache_key):
        # Re-check after acquiring the lock: another coroutine may have filled
        # the cache while this request waited behind the same cache key.
        cached_response = cache_manager.get(cache_key)
        if cached_response is not None:
            stats_collector.record_cache_hit(agent_name)
            return _build_cached_response(agent_name, cached_response)

        stats_collector.record_cache_miss()

        # Admission is deliberately after cache/coalescing so cache hits never
        # consume scarce agent capacity.
        limit_status = rate_limiter.check_and_record(agent_name)
        if not limit_status["allowed"]:
            return await _queue_overloaded_request(
                agent_name,
                query,
                cache_key,
                limit_status,
            )

        return await _process_agent_cache_miss(agent_name, query, cache_key, agent)


async def _process_queued_cache_miss_with_lock(
    queued_request: QueuedRequest,
) -> ResponseModel:
    """
    Apply cache-miss coalescing while the background worker drains queued work.

    This prevents queued and live requests for the same key from duplicating
    agent execution.
    """
    async with cache_key_lock(queued_request.cache_key):
        cached_response = cache_manager.get(queued_request.cache_key)
        if cached_response is not None:
            stats_collector.record_cache_hit(queued_request.agent)
            return _build_cached_response(
                queued_request.agent,
                cached_response,
            )

        await _wait_for_agent_capacity(queued_request.agent)
        return await _process_agent_cache_miss(
            queued_request.agent,
            queued_request.query,
            queued_request.cache_key,
        )


async def _wait_for_agent_capacity(agent_name: str) -> dict:
    """
    Wait until the limiter can admit queued work, then reserve capacity.

    The route counts rejected requests. The worker polls without increasing
    rejected-request metrics because it is draining already-admitted queue work.
    """
    while True:
        limit_status = rate_limiter.check_and_record(
            agent_name,
            count_rejected=False,
        )
        if limit_status["allowed"]:
            return limit_status

        retry_after = limit_status.get("retry_after", 0.25)
        sleep_seconds = min(max(retry_after, 0.25), 1.0)
        await asyncio.sleep(sleep_seconds)


async def process_queued_request(queued_request: QueuedRequest) -> dict:
    """
    Process one queued request in the background worker.

    This callback keeps queue infrastructure isolated while still letting the
    application layer own cache, limiter, agent, and stats behavior.
    """
    # QueueManager owns scheduling; this callback owns business processing.
    # Keeping the boundary here makes a future Redis/Celery/Kafka worker a
    # drop-in replacement for the in-memory queue loop.
    _sync_queue_observability()
    stats_collector.increment_active_requests()

    try:
        cached_response = cache_manager.get(queued_request.cache_key)
        if cached_response is not None:
            stats_collector.record_cache_hit(queued_request.agent)
            response = _build_cached_response(
                queued_request.agent,
                cached_response,
            )
        else:
            response = await _process_queued_cache_miss_with_lock(queued_request)

        stats_collector.record_queue_processed()
        return response.model_dump()
    finally:
        stats_collector.decrement_active_requests()
        _sync_queue_observability()


async def _queue_overloaded_request(
    agent_name: str,
    query: str,
    cache_key: str,
    limit_status: dict,
) -> JSONResponse:
    """
    Convert an over-limit cache miss into a queued API response.

    The route remains responsive under burst traffic while the background
    worker drains requests at the limiter's pace.
    """
    stats_collector.record_rate_limited(agent_name)

    queue_admission = await queue_manager.enqueue_request(
        agent=agent_name,
        query=query,
        cache_key=cache_key,
    )
    _sync_queue_observability()

    if not queue_admission.accepted:
        stats_collector.record_queue_overflow(
            queue_size=queue_admission.queue_size,
            max_queue_size=queue_admission.max_queue_size,
        )
        return JSONResponse(
            status_code=503,
            content={
                "status": queue_admission.status,
                "message": queue_admission.message,
                "queue_size": queue_admission.queue_size,
                "max_queue_size": queue_admission.max_queue_size,
                "estimated_wait_time": queue_admission.estimated_wait_time,
                "agent_limit_info": limit_status,
            },
        )

    stats_collector.record_queue_admission(
        queue_size=queue_admission.queue_size,
        max_queue_size=queue_admission.max_queue_size,
        estimated_wait_time=queue_admission.estimated_wait_time,
    )
    return JSONResponse(
        status_code=202,
        content={
            "status": queue_admission.status,
            "request_id": queue_admission.request_id,
            "agent": agent_name,
            "queue_position": queue_admission.queue_position,
            "estimated_wait_time": queue_admission.estimated_wait_time,
            "queue_size": queue_admission.queue_size,
            "message": queue_admission.message,
            "agent_limit_info": limit_status,
        },
    )


def _sync_queue_observability() -> dict:
    """Refresh queue gauges before returning monitoring payloads."""
    queue_status = queue_manager.get_queue_status()
    stats_collector.set_queue_status(
        queue_size=queue_status["queue_size"],
        max_queue_size=queue_status["max_queue_size"],
    )
    return queue_status


# ============================================================================
# LIFECYCLE MANAGEMENT
# ============================================================================

@asynccontextmanager
async def lifespan(app: FastAPI):
    """
    Application lifespan context manager.
    
    Starts the background queue worker and stops it during application shutdown.
    
    EXTENSION POINT:
    - Initialize Redis connections
    - Start external worker processes
    - Load cache warming strategies
    - Connect to observability backends
    """
    logger.info("AI Request Management Middleware starting up")
    logger.info("Metrics collector initialized")
    logger.info("Cache manager ready: in-memory cache")
    logger.info("Rate limiter ready: in-memory per-agent limiter")
    queue_manager.start_worker(process_queued_request)
    logger.info("Queue manager ready: in-memory async worker")

    try:
        yield
    finally:
        await queue_manager.stop_worker()
        logger.info("AI Request Management Middleware shut down")
        logger.info(
            "Final metrics summary: %s",
            stats_collector.get_metrics_summary(),
        )


# ============================================================================
# FASTAPI APPLICATION SETUP
# ============================================================================

app = FastAPI(
    title="AI Request Management Middleware",
    description=(
        "Resilient FastAPI middleware for cached, rate-limited, "
        "queued AI agent orchestration."
    ),
    version="1.0.0",
    lifespan=lifespan,
)

# Add CORS middleware for local development
# EXTENSION POINT (Phase 3): Add authentication and restrict origins
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],  # Local demo default; restrict origins before deployment.
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


# ============================================================================
# MIDDLEWARE
# ============================================================================

@app.middleware("http")
async def add_process_time_header(request: Request, call_next):
    """
    Add processing time tracking to all requests.
    
    EXTENSION POINT (Phase 2):
    - Add request ID correlation
    - Add distributed tracing headers
    - Add request validation middleware
    """
    start_time = time.perf_counter()
    response = await call_next(request)
    process_time = time.perf_counter() - start_time
    response.headers["X-Process-Time"] = str(process_time)
    return response


# ============================================================================
# ENDPOINTS - HEALTH & STATUS
# ============================================================================

@app.get("/", response_model=StatusModel, tags=["Health"])
async def root() -> StatusModel:
    """
    Welcome endpoint.
    
    Returns:
        Welcome message and API information
    """
    return StatusModel(
        status="ready",
        message="AI Request Management Middleware v1.0.0"
    )


@app.get("/health", tags=["Health"])
async def health_check() -> dict:
    """
    Operational health endpoint.

    Returns system health, warnings, bottlenecks, and demo-friendly
    recommendations from the observability service.
    """
    _sync_queue_observability()
    return stats_collector.get_system_health()


# ============================================================================
# ENDPOINTS - CORE FUNCTIONALITY
# ============================================================================

@app.post(
    "/request",
    response_model=ResponseModel,
    responses=REQUEST_RESPONSE_DOCS,
    tags=["Processing"],
)
async def process_request(
    request_payload: RequestModel,
) -> ResponseModel | JSONResponse:
    """
    Process an AI request through the orchestration pipeline.
    
    REQUEST PIPELINE:
    1. Validation - Check agent exists
    2. Increment Metrics - Track active and per-agent requests
    3. Cache Lookup - Check if response is cached
    4. Rate Limiter Check - Protect agents from overload
    5. Queue Admission (if overloaded) - Buffer traffic spikes
    6. Agent Processing (if allowed cache miss) - Route to agent
    7. Store in Cache - Save newly generated response
    8. Response Assembly - Package response with metrics
    9. Cleanup - Decrement active request count
    
    CACHE BEHAVIOR:
    - Cache Hit: Return immediately (~1-5ms), cached=True
    - Cache Miss: Process normally (~500-700ms), cached=False
    - Rate Limited: Enqueue request and return HTTP 202, status=queued
    - Queue Full: Return HTTP 503 with queue status
    
    EXTENSION POINTS:
    - Phase 4+: Replace in-memory cache with Redis
    - Phase 5: In-memory queue buffer for overload smoothing
    - Phase 6: Add distributed tracing and observability
    
    Args:
        request_payload: Request model with agent and query
        
    Returns:
        ResponseModel with agent response and processing metrics
        
    Raises:
        HTTPException: If agent not found or validation fails
    """
    # Validation is intentionally first so invalid agent/query values never
    # touch cache keys, limiter state, queue state, or metrics.
    agent_name, query = _normalize_request(request_payload)
    agent = _get_agent_or_404(agent_name)

    # Track visible traffic pressure before the request either completes
    # immediately, gets queued, or enters agent processing.
    stats_collector.record_request_started(agent_name)
    
    try:
        # Cache lookup is first so repeated prompts bypass both rate limiting
        # and queueing, demonstrating the optimization clearly.
        cache_key = generate_cache_key(agent_name, query)
        cached_response = cache_manager.get(cache_key)

        if cached_response is not None:
            # CACHE HIT: Return immediately without agent admission or queueing.
            stats_collector.record_cache_hit(agent_name)
            return _build_cached_response(agent_name, cached_response)
        
        # Cache misses move into the coalescing path: only the first identical
        # miss reaches limiter/agent work while followers wait for the cache.
        return await _handle_cache_miss_with_lock(agent_name, query, cache_key, agent)
    finally:
        stats_collector.record_request_finished()


# ============================================================================
# ENDPOINTS - METRICS & MONITORING
# ============================================================================

@app.get("/stats", tags=["Metrics"])
async def get_stats() -> dict:
    """
    Get current observability snapshot.
    
    Returns grouped traffic, cache, queue, performance, per-agent, and health
    metrics. This shape is intentionally dashboard-friendly while staying
    lightweight and in-memory for hackathon demos.
    
    EXTENSION POINT:
    - Export this structured snapshot to Prometheus/Grafana later
    - Add time-window aggregation and percentiles
    - Attach trace IDs to request-level events
    """
    _sync_queue_observability()
    return stats_collector.get_metrics()


@app.post("/stats/reset", tags=["Metrics"])
async def reset_stats() -> dict:
    """
    Reset observability counters without clearing cache, queue, or limiter state.

    This is useful for demos: start with clean charts while preserving system
    stability and any in-flight background worker state.
    """
    stats_collector.reset_metrics()
    _sync_queue_observability()
    return {
        "status": "reset",
        "message": "In-memory observability metrics reset safely.",
        "metrics": stats_collector.get_metrics_summary(),
    }


@app.get("/metrics/summary", tags=["Metrics"])
async def get_metrics_summary() -> dict:
    """
    Get a compact dashboard-friendly metrics summary.

    Highlights the resilience signals people care about during a demo:
    queue depth, cache ratio, limiter pressure, active requests, and health.
    """
    _sync_queue_observability()
    return stats_collector.get_metrics_summary()


@app.get("/queue/status", tags=["Metrics"])
async def get_queue_status() -> dict:
    """
    Get live queue depth and worker status.

    Shows how the in-memory resilience buffer is absorbing overload traffic.
    """
    return _sync_queue_observability()


# ============================================================================
# ENDPOINTS - AGENT MANAGEMENT
# ============================================================================

@app.get("/agents", tags=["Agents"])
async def list_agents() -> dict:
    """
    List all available AI agents.
    
    Returns information about each registered agent including:
    - Name and description
    - Capabilities
    - Request statistics
    
    EXTENSION POINT (Phase 3):
    - Add agent health status
    - Add per-agent metrics
    - Add agent scaling information
    
    Returns:
        Dictionary mapping agent names to their information
    """
    return {
        "agents": agent_registry.list_agents(),
        "total_agents": len(agent_registry.agents)
    }


@app.get("/agents/status", tags=["Agents"])
async def get_agents_status() -> dict:
    """
    Get per-agent operational observability.

    Shows request volume, cache hits, limiter pressure, and average processing
    time for each observed agent.
    """
    return stats_collector.get_agents_status()


@app.get("/agents/{agent_name}", tags=["Agents"])
async def get_agent_info(agent_name: str) -> dict:
    """
    Get detailed information about a specific agent.
    
    Args:
        agent_name: Name of the agent to retrieve
        
    Returns:
        Agent details and statistics
        
    Raises:
        HTTPException: If agent not found
    """
    try:
        agent_stats = agent_registry.get_agent_stats(agent_name)
        return {
            "status": "found",
            "agent": agent_stats
        }
    except KeyError as e:
        raise HTTPException(status_code=404, detail=str(e))


# ============================================================================
# ENDPOINTS - COMPONENT STATUS (Extension Points)
# ============================================================================

@app.get("/debug/cache", tags=["Debug"])
async def debug_cache() -> dict:
    """
    Debug endpoint for cache status and contents.
    
    Shows cache statistics, hit rates, and cached entries.
    
    EXTENSION POINT (Phase 4):
    - Add Redis backend information
    - Add cache eviction statistics
    - Add per-agent cache breakdown
    """
    return {
        "cache_stats": cache_manager.get_stats(),
        "note": "Phase 3: In-memory cache. Phase 4: Will use Redis for distributed caching."
    }


@app.get("/debug/queue", tags=["Debug"])
async def debug_queue() -> dict:
    """
    Debug endpoint for queue status.
    
    EXTENSION POINT (Phase 3):
    - Restrict to authenticated admin users
    - Add queue inspection endpoint
    - Add dead-letter queue viewer
    """
    return {
        "queue_stats": queue_manager.get_queue_status(),
        "note": (
            "Phase 5: In-memory async queue. Future phases can replace it "
            "with Redis/Celery/Kafka."
        ),
    }


@app.get("/debug/limiter", tags=["Debug"])
async def debug_limiter() -> dict:
    """
    Debug endpoint for rate limiter status.
    
    EXTENSION POINT (Phase 4):
    - Restrict to authenticated admin users
    - Add per-agent thresholds and dynamic adjustment
    - Add admin controls for rate limit tuning
    """
    return {
        "limiter_stats": rate_limiter.get_stats(),
        "note": "Phase 4: In-memory per-agent limiter. Phase 5: Replace with distributed limiter."
    }


@app.get("/limits", tags=["Limits"])
async def get_limits() -> dict:
    """
    Get current rate limit configuration and active usage.
    
    Returns current per-agent limit status, remaining capacity, and retry windows.
    """
    return {
        "limits": rate_limiter.get_limits()
    }


@app.get("/debug/agents", tags=["Debug"])
async def debug_agents() -> dict:
    """
    Debug endpoint for agent registry and statistics.
    
    Shows all registered agents and their processing statistics.
    
    EXTENSION POINT (Phase 3):
    - Add real-time agent performance gauges
    - Add agent error tracking
    - Add agent throughput metrics
    """
    agents_info = agent_registry.list_agents()
    
    return {
        "agents": agents_info,
        "total_agents": len(agents_info),
        "note": "Phase 2: Fake agents. Phase 3: Real LLM integration."
    }


# ============================================================================
# ENDPOINTS - CACHE MANAGEMENT
# ============================================================================

@app.post("/cache/clear", tags=["Cache Management"])
async def clear_cache() -> dict:
    """
    Clear all cached responses.
    
    This endpoint safely clears the in-memory cache dictionary,
    removing all previously cached agent responses.
    
    Useful for:
    - Forcing fresh computation
    - Resetting metrics for testing
    - Memory management during long-running sessions
    
    EXTENSION POINT (Phase 4):
    - Add authentication requirement
    - Add granular invalidation (by agent, by age, etc.)
    - Add pre-invalidation hooks
    
    Returns:
        Dictionary with clear operation status
    """
    initial_size = len(cache_manager.store)
    cache_manager.clear()
    final_size = len(cache_manager.store)
    
    return {
        "status": "success",
        "message": f"Cache cleared. Removed {initial_size} entries.",
        "cache_size_before": initial_size,
        "cache_size_after": final_size
    }


# ============================================================================
# ERROR HANDLING
# ============================================================================

@app.exception_handler(HTTPException)
async def http_exception_handler(request: Request, exc: HTTPException):
    """
    Custom HTTP exception handler.
    
    EXTENSION POINT (Phase 2):
    - Add distributed tracing context
    - Add error logging to observability backend
    - Add retry suggestions
    """
    return JSONResponse(
        status_code=exc.status_code,
        content={
            "error": exc.detail,
            "status_code": exc.status_code,
            "request_path": request.url.path
        }
    )


# ============================================================================
# ENTRY POINT
# ============================================================================

if __name__ == "__main__":
    import uvicorn
    
    # Run with: uvicorn main:app --host 0.0.0.0 --port 8000 --reload
    uvicorn.run(
        app,
        host="0.0.0.0",
        port=8000,
        log_level="info"
    )
