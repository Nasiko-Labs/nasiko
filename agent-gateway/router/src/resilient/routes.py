"""
REST API routes for resilient request layer.
Provides monitoring and operational endpoints.
"""

from fastapi import APIRouter, HTTPException, Query, Body
from typing import Optional, Dict, Any

from router.src.resilient import ResilientRequestLayer

# This will be instantiated and passed to the router
resilient_router = APIRouter(prefix="/router/resilient", tags=["resilient"])


class ResilientRequestLayerAPI:
    """API wrapper for resilient request layer operations."""

    def __init__(self, request_layer: ResilientRequestLayer):
        """Initialize with request layer instance."""
        self.layer = request_layer
        self._setup_routes()

    def _setup_routes(self):
        """Setup all API routes."""
        
        @resilient_router.get("/health")
        async def health_check():
            """Check health of all resilient layer components."""
            return self.layer.health()

        @resilient_router.get("/cache/stats")
        async def get_cache_stats(agent_id: Optional[str] = Query(None)):
            """Get cache statistics for all agents or specific agent."""
            try:
                if agent_id:
                    return {"agent": agent_id, "stats": self.layer.cache.stats(agent_id)}
                else:
                    return {"agents": self.layer.cache.all_stats()}
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.post("/cache/ttl")
        async def get_cache_ttl(
            agent_id: str,
            request_data: Dict[str, Any] = Body(...),
        ):
            """Get remaining TTL in seconds for a cached request."""
            try:
                return self.layer.cache.ttl(agent_id, request_data)
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.post("/cache/clear")
        async def clear_cache(agent_id: Optional[str] = None):
            """Clear cache for specific agent or all agents."""
            try:
                if agent_id:
                    deleted = self.layer.cache.flush_agent(agent_id)
                    return {"agent": agent_id, "deleted_count": deleted}
                else:
                    deleted = self.layer.cache.flush_all()
                    return {"deleted_count": deleted, "message": "All cache cleared"}
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.get("/rate-limit/config")
        async def get_rate_limit_config():
            """Get rate limit configuration for all agents."""
            try:
                return {"agents": self.layer.rate_limiter.get_all_configs()}
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.get("/rate-limit/state")
        async def get_rate_limit_state(agent_id: str):
            """Get current rate limit state for an agent."""
            try:
                return {
                    "agent": agent_id,
                    "state": self.layer.rate_limiter.get_current_state(agent_id)
                }
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.post("/rate-limit/update")
        async def update_rate_limit(
            agent_id: str,
            requests_per_second: float = 10.0,
            burst_capacity: int = 50,
        ):
            """Update rate limit for an agent."""
            try:
                success = self.layer.rate_limiter.set_default_limit(
                    agent_id,
                    requests_per_second,
                    burst_capacity
                )
                if success:
                    return {
                        "agent": agent_id,
                        "rps": requests_per_second,
                        "burst": burst_capacity,
                        "status": "updated"
                    }
                else:
                    raise HTTPException(status_code=500, detail="Failed to update rate limit")
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.post("/rate-limit/reset")
        async def reset_rate_limit(agent_id: str):
            """Reset rate limiter for an agent to full capacity."""
            try:
                success = self.layer.rate_limiter.reset_agent(agent_id)
                if success:
                    return {"agent": agent_id, "status": "reset", "reset": True}
                else:
                    raise HTTPException(status_code=500, detail="Failed to reset rate limit")
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.get("/queue/status")
        async def get_queue_status():
            """Get queue status for all agents."""
            try:
                return {"agents": self.layer.queue.get_all_queue_status()}
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.get("/queue/status/{agent_id}")
        async def get_agent_queue_status(agent_id: str):
            """Get queue status for a specific agent."""
            try:
                return {
                    "agent": agent_id,
                    "status": self.layer.queue.get_queue_status(agent_id)
                }
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.post("/queue/process")
        async def process_queue(
            agent_id: str,
            max_requests: int = 10,
        ):
            """Manually trigger queue processing for an agent."""
            try:
                processed = self.layer.process_queue(agent_id, max_requests)
                return {
                    "agent": agent_id,
                    "processed_count": processed,
                    "status": "queue_processed"
                }
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.post("/queue/clear")
        async def clear_queue(agent_id: str):
            """Clear all queued requests for an agent."""
            try:
                cleared = self.layer.queue.clear_queue(agent_id)
                return {
                    "agent": agent_id,
                    "cleared_count": cleared,
                    "status": "queue_cleared"
                }
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.get("/metrics/stats")
        async def get_metrics_stats():
            """Get comprehensive metrics for all agents."""
            try:
                return self.layer.metrics.get_summary()
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.get("/metrics/stats/{agent_id}")
        async def get_agent_metrics(agent_id: str):
            """Get metrics for a specific agent."""
            try:
                return {
                    "agent": agent_id,
                    "metrics": self.layer.metrics.get_stats(agent_id)
                }
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.post("/metrics/reset")
        async def reset_metrics(agent_id: Optional[str] = None):
            """Reset metrics for specific agent or all agents."""
            try:
                if agent_id:
                    success = self.layer.metrics.reset_stats(agent_id)
                    return {"agent": agent_id, "reset": success}
                else:
                    count = self.layer.metrics.reset_all_stats()
                    return {"agents_reset": count, "reset": True}
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.post("/agent/reset")
        async def reset_agent(agent_id: str):
            """Reset all state for an agent (cache, rate limit, queue, metrics)."""
            try:
                cache_flushed = self.layer.cache.flush_agent(agent_id)
                rate_limit_reset = self.layer.rate_limiter.reset_agent(agent_id)
                queue_cleared = self.layer.queue.clear_queue(agent_id)
                metrics_reset = self.layer.metrics.reset_stats(agent_id)
                
                return {
                    "agent": agent_id,
                    "status": "all_reset",
                    "cache_flushed": cache_flushed,
                    "rate_limit_reset": rate_limit_reset,
                    "queue_cleared": queue_cleared,
                    "metrics_reset": metrics_reset
                }
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.post("/agent/configure")
        async def configure_agent(
            agent_id: str,
            requests_per_second: Optional[float] = None,
            burst_capacity: Optional[int] = None,
            cache_ttl_seconds: Optional[int] = None,
            max_queue_size: Optional[int] = None,
        ):
            """Configure rate limiting and caching for an agent."""
            try:
                success = self.layer.configure_agent(
                    agent_id,
                    requests_per_second,
                    burst_capacity,
                    cache_ttl_seconds,
                    max_queue_size
                )
                if success:
                    return {
                        "agent": agent_id,
                        "status": "configured",
                        "config": {
                            "requests_per_second": requests_per_second,
                            "burst_capacity": burst_capacity,
                            "cache_ttl_seconds": cache_ttl_seconds,
                            "max_queue_size": max_queue_size,
                        }
                    }
                else:
                    raise HTTPException(status_code=500, detail="Failed to configure agent")
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))

        @resilient_router.get("/dashboard")
        async def get_dashboard():
            """Get comprehensive dashboard data for all systems."""
            try:
                return self.layer.get_comprehensive_stats()
            except Exception as e:
                raise HTTPException(status_code=500, detail=str(e))


def create_resilient_routes(request_layer: ResilientRequestLayer) -> APIRouter:
    """
    Create and configure the resilient request layer API router.
    
    Args:
        request_layer: ResilientRequestLayer instance
        
    Returns:
        Configured FastAPI APIRouter
    """
    api = ResilientRequestLayerAPI(request_layer)
    return resilient_router
