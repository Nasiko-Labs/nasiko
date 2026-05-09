from datetime import datetime
from typing import Dict, Any


class MetricsCollector:
    """Collects and tracks all operational metrics"""
    
    def __init__(self):
        self.total_requests = 0
        self.cache_hits = 0
        self.cache_misses = 0
        self.coalesced = 0
        self.queued = 0
        self.agent_calls = 0
        self.agent_errors = 0
        self.success_count = 0
        self.error_count = 0
        
        # Latency tracking
        self.latency_sum = 0.0
        self.min_latency = float('inf')
        self.max_latency = 0.0
        
        self.start_time = datetime.utcnow()
    
    def increment_request(self):
        self.total_requests += 1
    
    def record_cache_hit(self):
        self.cache_hits += 1
    
    def record_cache_miss(self):
        self.cache_misses += 1
    
    def record_coalesced(self):
        self.coalesced += 1
    
    def record_queued(self):
        self.queued += 1
    
    def record_agent_call(self):
        self.agent_calls += 1
    
    def record_agent_error(self):
        self.agent_errors += 1
    
    def record_success(self):
        self.success_count += 1
    
    def record_error(self):
        self.error_count += 1
    
    def record_latency(self, latency_ms: float):
        """Record request latency"""
        self.latency_sum += latency_ms
        self.min_latency = min(self.min_latency, latency_ms)
        self.max_latency = max(self.max_latency, latency_ms)
    
    def get_average_latency(self) -> float:
        """Get average latency in milliseconds"""
        if self.total_requests == 0:
            return 0.0
        return self.latency_sum / self.total_requests
    
    def get_request_stats(self) -> Dict[str, Any]:
        """Get comprehensive request statistics"""
        
        uptime = (datetime.utcnow() - self.start_time).total_seconds()
        
        # Calculate optimization ratio
        optimized = self.cache_hits + self.coalesced
        total = max(self.total_requests, 1)
        
        return {
            "total_requests": self.total_requests,
            "requests_per_second": round(self.total_requests / max(uptime, 1), 2),
            "success_count": self.success_count,
            "error_count": self.error_count,
            "cache_hits": self.cache_hits,
            "cache_misses": self.cache_misses,
            "coalesced": self.coalesced,
            "queued": self.queued,
            "agent_calls": self.agent_calls,
            "agent_errors": self.agent_errors,
            "optimization_ratio": f"{(optimized / total * 100):.1f}%",
            "uptime_seconds": round(uptime, 2),
            "latency": {
                "average_ms": round(self.get_average_latency(), 2),
                "min_ms": round(self.min_latency if self.min_latency != float('inf') else 0, 2),
                "max_ms": round(self.max_latency, 2)
            }
        }
