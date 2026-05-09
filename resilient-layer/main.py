from fastapi import FastAPI, HTTPException, Query, Body
from fastapi.responses import JSONResponse
from pydantic import BaseModel
from contextlib import asynccontextmanager
import asyncio
from datetime import datetime
from typing import Optional, Dict, Any
import hashlib
import httpx
import uuid
import json
import os
import logging

from cache_engine import CacheEngine
from rate_limiter import RateLimiter
from coalescer import RequestCoalescer
from queue_manager import QueueManager
from metrics import MetricsCollector

logger = logging.getLogger("resilient-layer")
logging.basicConfig(level=logging.INFO)

# Global instances
cache = CacheEngine()
limiter = RateLimiter()
coalescer = RequestCoalescer()
queue_mgr = QueueManager()
metrics = MetricsCollector()

# Agent configurations
AGENT_CONFIGS = {
    "translator": {"rate_limit": 10, "window": 1, "priority": 2},
    "compliance-checker": {"rate_limit": 5, "window": 1, "priority": 3},
    "github-agent": {"rate_limit": 8, "window": 1, "priority": 1},
}

# Service URLs
ROUTER_URL = os.getenv("ROUTER_URL", "http://nasiko-router:8000")
AUTH_SERVICE_URL = os.getenv("AUTH_SERVICE_URL", "http://nasiko-auth-service:8001")


class TokenManager:
    """Manages JWT token lifecycle for service-to-service auth."""

    def __init__(self, auth_url: str):
        self.auth_url = auth_url
        self._token: Optional[str] = None
        self._access_key: Optional[str] = None
        self._access_secret: Optional[str] = None
        self._refresh_task: Optional[asyncio.Task] = None

    async def initialize(self):
        """Register a service user and obtain initial token."""
        async with httpx.AsyncClient(timeout=15.0) as client:
            # Register a service user for resilient-layer
            try:
                reg_resp = await client.post(
                    f"{self.auth_url}/auth/users/register",
                    json={
                        "username": "resilient-layer-svc",
                        "email": "resilient-layer@nasiko.internal",
                        "is_super_user": False,
                    },
                )
                if reg_resp.status_code in (200, 201):
                    data = reg_resp.json()
                    self._access_key = data["access_key"]
                    self._access_secret = data["access_secret"]
                    logger.info("Registered resilient-layer service user")
                elif reg_resp.status_code == 400 and "already exists" in reg_resp.text.lower():
                    logger.info("Service user already exists, will use stored credentials")
                else:
                    logger.warning(f"Service user registration: {reg_resp.status_code} {reg_resp.text[:200]}")
            except Exception as e:
                logger.warning(f"Could not register service user: {e}")

            # If we have stored credentials from env, use those
            if not self._access_key:
                self._access_key = os.getenv("SERVICE_ACCESS_KEY")
                self._access_secret = os.getenv("SERVICE_ACCESS_SECRET")

            # Obtain a token
            await self._refresh_token()

        # Start periodic refresh (every 6 hours, tokens expire in 12h)
        self._refresh_task = asyncio.create_task(self._periodic_refresh())

    async def _refresh_token(self):
        """Get a fresh JWT from auth service."""
        if not self._access_key or not self._access_secret:
            logger.warning("No service credentials available, requests will be unauthenticated")
            return

        try:
            async with httpx.AsyncClient(timeout=15.0) as client:
                resp = await client.post(
                    f"{self.auth_url}/auth/users/login",
                    json={
                        "access_key": self._access_key,
                        "access_secret": self._access_secret,
                    },
                )
                if resp.status_code == 200:
                    data = resp.json()
                    self._token = data["token"]
                    logger.info("JWT token refreshed successfully")
                else:
                    logger.error(f"Token refresh failed: {resp.status_code} {resp.text[:200]}")
        except Exception as e:
            logger.error(f"Token refresh error: {e}")

    async def _periodic_refresh(self):
        """Refresh token every 6 hours."""
        while True:
            await asyncio.sleep(6 * 3600)
            await self._refresh_token()

    @property
    def auth_headers(self) -> Dict[str, str]:
        if self._token:
            return {"Authorization": f"Bearer {self._token}"}
        return {}


token_mgr = TokenManager(AUTH_SERVICE_URL)


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Startup and shutdown events"""
    # Initialize auth token
    try:
        await token_mgr.initialize()
    except Exception as e:
        logger.error(f"Token manager init failed (non-fatal): {e}")

    # Initialize rate limiters for known agents
    for agent_name, config in AGENT_CONFIGS.items():
        await limiter.add_agent(
            agent_name,
            rate=config["rate_limit"],
            window=config["window"]
        )
    
    # Start queue worker
    asyncio.create_task(queue_mgr.process_queue(lambda q, a: forward_to_router(q, a)))
    
    yield
    
    # Cleanup
    await cache.cleanup()
    await coalescer.cleanup()


app = FastAPI(
    title="Nasiko Resilient Layer - AgentShield",
    description="Intelligent traffic control for multi-agent AI systems",
    version="1.0.0",
    lifespan=lifespan
)


def generate_cache_key(query: str, agent_hint: Optional[str] = None) -> str:
    """Generate deterministic cache key"""
    content = f"{query}:{agent_hint or 'any'}"
    return hashlib.sha256(content.encode()).hexdigest()


async def forward_to_router(query: str, agent_hint: Optional[str] = None) -> Dict[str, Any]:
    """Forward request to Nasiko router via POST /router (multipart/form-data)"""
    try:
        async with httpx.AsyncClient(timeout=60.0) as client:
            # Build multipart form data matching the router's expected schema
            form_data = {
                "session_id": str(uuid.uuid4()),
                "query": query,
            }
            if agent_hint:
                form_data["route"] = agent_hint

            response = await client.post(
                f"{ROUTER_URL}/router",
                data=form_data,
                headers=token_mgr.auth_headers,
            )
            response.raise_for_status()

            metrics.record_agent_call()

            # The router may return a streaming response with multiple JSON events.
            # Parse the response text and extract meaningful content.
            raw_text = response.text.strip()

            # Try parsing as plain JSON first
            try:
                return response.json()
            except Exception:
                pass

            # Handle streaming / newline-delimited JSON
            results = []
            for line in raw_text.split("\n"):
                line = line.strip()
                if not line:
                    continue
                try:
                    parsed = json.loads(line)
                    results.append(parsed)
                except json.JSONDecodeError:
                    results.append({"text": line})

            if len(results) == 1:
                return results[0]
            elif results:
                # Combine streaming chunks — look for the final/meaningful one
                final = results[-1]
                return {
                    "response": final,
                    "chunks": len(results),
                }
            else:
                return {"response": raw_text}

    except httpx.HTTPStatusError as e:
        metrics.record_agent_error()
        raise HTTPException(
            status_code=502,
            detail=f"Router error ({e.response.status_code}): {e.response.text[:500]}"
        )
    except Exception as e:
        metrics.record_agent_error()
        raise HTTPException(status_code=502, detail=f"Router error: {str(e)}")


@app.get("/health")
async def health_check():
    """Health check endpoint"""
    return {
        "status": "healthy",
        "timestamp": datetime.utcnow().isoformat(),
        "service": "resilient-layer"
    }


class ProcessRequestBody(BaseModel):
    query: Optional[str] = None
    agent_hint: Optional[str] = None
    priority: Optional[int] = None


@app.post("/process")
async def process_request(
    query: Optional[str] = Query(None),
    agent_hint: Optional[str] = Query(None),
    priority: Optional[int] = Query(None),
    request_body: Optional[ProcessRequestBody] = Body(None),
):
    """
    Main request processing endpoint with all intelligence layers.
    
    Flow:
    1. Check if identical request is being processed (coalescing)
    2. Check cache for similar results
    3. Apply rate limiting
    4. If overloaded, queue the request
    5. Forward to agent/router
    6. Cache result and notify waiting requests
    """
    if request_body:
        query = query or request_body.query
        agent_hint = agent_hint or request_body.agent_hint
        priority = priority or request_body.priority

    if not query:
        raise HTTPException(status_code=400, detail="query parameter is required")
    
    request_start = datetime.utcnow()
    metrics.increment_request()
    
    # STEP 1: Request Coalescing (HERO FEATURE)
    pending_key = generate_cache_key(query, agent_hint)
    
    existing_waiter = await coalescer.register_if_pending(pending_key)
    if existing_waiter is not None:
        metrics.record_coalesced()
        result = await coalescer.wait_for_result(pending_key, timeout=10.0)
        if result:
            elapsed = (datetime.utcnow() - request_start).total_seconds() * 1000
            metrics.record_latency(elapsed)
            return JSONResponse(content={
                "source": "coalesced",
                "result": result,
                "latency_ms": elapsed,
                "saved_computation": True
            })
    
    # STEP 2: Cache Check
    cached_result, similarity = await cache.lookup(query, agent_hint)
    
    if cached_result and similarity > 0.95:
        metrics.record_cache_hit()
        await coalescer.complete_request(pending_key, cached_result)
        elapsed = (datetime.utcnow() - request_start).total_seconds() * 1000
        metrics.record_latency(elapsed)
        return JSONResponse(content={
            "source": "cache",
            "similarity": similarity,
            "result": cached_result,
            "latency_ms": elapsed
        })
    
    # STEP 3: Rate Limiting
    if agent_hint and agent_hint in AGENT_CONFIGS:
        rate_allowed = await limiter.acquire(agent_hint)
        
        if not rate_allowed:
            metrics.record_queued()
            queue_entry = await queue_mgr.enqueue(
                query=query,
                agent_hint=agent_hint,
                priority=priority or AGENT_CONFIGS[agent_hint]["priority"],
                pending_key=pending_key
            )
            
            result = await queue_mgr.wait_for_processing(pending_key, timeout=30.0)
            if result:
                elapsed = (datetime.utcnow() - request_start).total_seconds() * 1000
                metrics.record_latency(elapsed)
                return JSONResponse(content={
                    "source": "queued",
                    "result": result,
                    "queue_position": queue_entry["position"],
                    "latency_ms": elapsed
                })
    
    # STEP 4: Forward to Router/Agent
    try:
        agent_response = await forward_to_router(query, agent_hint)
        
        # STEP 5: Cache the result
        await cache.store(query, agent_response, agent_hint)
        
        # STEP 6: Complete coalescing
        await coalescer.complete_request(pending_key, agent_response)
        
        metrics.record_success()
        elapsed = (datetime.utcnow() - request_start).total_seconds() * 1000
        metrics.record_latency(elapsed)
        
        return JSONResponse(content={
            "source": "agent",
            "result": agent_response,
            "latency_ms": elapsed
        })
        
    except Exception as e:
        metrics.record_error()
        await coalescer.complete_request(pending_key, {"error": str(e)})
        raise


@app.post("/router")
async def router_post_alias(
    query: Optional[str] = Query(None),
    agent_hint: Optional[str] = Query(None),
    priority: Optional[int] = Query(None),
    request_body: Optional[ProcessRequestBody] = Body(None),
):
    """Compatibility alias for /router to preserve existing Nasiko routing semantics."""
    if request_body:
        query = query or request_body.query
        agent_hint = agent_hint or request_body.agent_hint
        priority = priority or request_body.priority
    return await process_request(query=query, agent_hint=agent_hint, priority=priority)


@app.get("/router/route")
async def router_get_route(
    query: Optional[str] = Query(None),
    agent_hint: Optional[str] = Query(None),
    priority: Optional[int] = Query(None),
):
    """Compatibility alias for legacy GET router route calls."""
    if not query:
        raise HTTPException(status_code=400, detail="query parameter is required")
    return await process_request(query=query, agent_hint=agent_hint, priority=priority)


@app.post("/router/route")
async def router_post_route(
    query: Optional[str] = Query(None),
    agent_hint: Optional[str] = Query(None),
    priority: Optional[int] = Query(None),
    request_body: Optional[ProcessRequestBody] = Body(None),
):
    """Compatibility alias for legacy router route POST calls."""
    if request_body:
        query = query or request_body.query
        agent_hint = agent_hint or request_body.agent_hint
        priority = priority or request_body.priority
    if not query:
        raise HTTPException(status_code=400, detail="query parameter is required")
    return await process_request(query=query, agent_hint=agent_hint, priority=priority)


@app.get("/metrics")
async def get_metrics():
    """Comprehensive metrics endpoint"""
    return {
        "requests": metrics.get_request_stats(),
        "cache": await cache.get_stats(),
        "coalescing": await coalescer.get_stats(),
        "queue": await queue_mgr.get_stats(),
        "rate_limits": await limiter.get_stats(),
        "timestamp": datetime.utcnow().isoformat()
    }


@app.get("/cache/stats")
async def cache_stats():
    """Cache-specific statistics"""
    return await cache.get_stats()


@app.post("/cache/clear")
async def clear_cache():
    """Clear entire cache"""
    await cache.clear()
    return {"status": "cleared"}


@app.get("/queue/status")
async def queue_status():
    """Current queue status"""
    return await queue_mgr.get_stats()


@app.get("/limits")
async def get_rate_limits():
    """Get current rate limit configurations"""
    return await limiter.get_stats()


@app.post("/limits/update")
async def update_rate_limit(
    agent_name: str,
    new_rate: int = Query(..., gt=0),
    window: int = Query(1, gt=0)
):
    """Dynamically update rate limit for an agent"""
    await limiter.update_limit(agent_name, new_rate, window)
    return {
        "agent": agent_name,
        "new_rate": new_rate,
        "window": window,
        "status": "updated"
    }
