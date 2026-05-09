import redis
import time
from typing import Dict, Any, List

class MetricsCollector:
    def __init__(self, redis_host: str = "redis", redis_port: int = 6379):
        self.redis = redis.Redis(
            host=redis_host,
            port=redis_port,
            decode_responses=True,
            socket_connect_timeout=5,
            socket_timeout=5
        )
        self.llm_cost_per_call = 0.002  # $0.002 average per LLM call
        
    def get_all_metrics(self) -> Dict[str, Any]:
        agents = self._discover_agents()
        
        metrics = {
            "timestamp": time.time(),
            "service": "resilience-layer",
            "agents": {}
        }
        
        for agent in agents:
            metrics["agents"][agent] = self._get_agent_metrics(agent)
        
        metrics["summary"] = self._get_summary(metrics["agents"])
        return metrics
    
    def _discover_agents(self) -> List[str]:
        """Discover agents from Redis keys"""
        patterns = ["cache:*", "ratelimit:*", "circuit:*"]
        agents = set()
        
        for pattern in patterns:
            keys = self.redis.keys(pattern)
            for key in keys:
                parts = key.split(":")
                if len(parts) >= 2:
                    agents.add(parts[1])
        
        return list(agents) if agents else ["translator"]
    
    def _get_agent_metrics(self, agent: str) -> Dict[str, Any]:
        # Cache metrics
        cache_keys = self.redis.keys(f"cache:{agent}:*")
        cache_total = len(cache_keys)
        cache_hits = 0
        for key in cache_keys:
            if self.redis.ttl(key) > 0:
                cache_hits += 1
        
        total_requests = int(self.redis.get(f"total:{agent}") or 0)
        cache_hit_rate = cache_hits / max(total_requests, 1)
        
        # Rate limit metrics
        tokens = self.redis.get(f"ratelimit:{agent}:tokens")
        queue_p0 = self.redis.llen(f"queue:{agent}:P0")
        queue_p1 = self.redis.llen(f"queue:{agent}:P1")
        queue_p2 = self.redis.llen(f"queue:{agent}:P2")
        
        # Circuit metrics
        circuit_errors = int(self.redis.get(f"circuit:{agent}:errors") or 0)
        circuit_total = int(self.redis.get(f"circuit:{agent}:total") or 1)
        circuit_state = self.redis.get(f"circuit:{agent}:state") or "CLOSED"
        
        # Cost savings
        cache_hits_total = int(self.redis.get(f"metrics:{agent}:cache_hits") or cache_hits)
        cost_saved = cache_hits_total * self.llm_cost_per_call
        
        return {
            "cache": {
                "total_entries": cache_total,
                "active_entries": cache_hits,
                "hit_rate": round(cache_hit_rate, 4),
                "cost_saved_usd": round(cost_saved, 4)
            },
            "rate_limit": {
                "tokens_remaining": int(tokens) if tokens else 0,
                "queue_depths": {
                    "P0": queue_p0,
                    "P1": queue_p1,
                    "P2": queue_p2
                },
                "total_queued": queue_p0 + queue_p1 + queue_p2
            },
            "circuit": {
                "state": circuit_state,
                "error_rate": round(circuit_errors / max(circuit_total, 1), 4),
                "total_requests": circuit_total,
                "error_requests": circuit_errors
            }
        }
    
    def _get_summary(self, agent_metrics: Dict[str, Any]) -> Dict[str, Any]:
        total_cache_hits = sum(m["cache"]["active_entries"] for m in agent_metrics.values())
        total_cost_saved = sum(m["cache"]["cost_saved_usd"] for m in agent_metrics.values())
        total_queued = sum(m["rate_limit"]["total_queued"] for m in agent_metrics.values())
        open_circuits = sum(1 for m in agent_metrics.values() if m["circuit"]["state"] == "OPEN")
        
        return {
            "total_agents": len(agent_metrics),
            "total_cache_hits": total_cache_hits,
            "total_cost_saved_usd": round(total_cost_saved, 4),
            "total_queued_requests": total_queued,
            "open_circuits": open_circuits,
            "healthy_circuits": len(agent_metrics) - open_circuits
        }

    def get_predictive_alerts(self) -> List[Dict[str, Any]]:
        """Predict when agents will hit rate limits or circuit breakers"""
        alerts = []
        agents = self._discover_agents()
        
        for agent in agents:
            # Predict rate limit exhaustion
            tokens = int(self.redis.get(f"ratelimit:{agent}:tokens") or 0)
            queue_p0 = self.redis.llen(f"queue:{agent}:P0")
            queue_p1 = self.redis.llen(f"queue:{agent}:P1")
            total_queued = queue_p0 + queue_p1
            
            if tokens < 20 and total_queued > 5:
                estimated_seconds = (tokens / max(total_queued, 1)) * 10  # refill every 10s
                alerts.append({
                    "agent": agent,
                    "type": "rate_limit_exhaustion",
                    "severity": "warning" if tokens > 0 else "critical",
                    "message": f"Agent '{agent}' will exhaust tokens in ~{estimated_seconds:.0f}s at current velocity",
                    "current_tokens": tokens,
                    "queued_requests": total_queued,
                    "estimated_exhaustion_seconds": round(estimated_seconds, 1)
                })
            
            # Predict circuit breaker
            errors = int(self.redis.get(f"circuit:{agent}:errors") or 0)
            total = int(self.redis.get(f"circuit:{agent}:total") or 1)
            error_rate = errors / total
            
            if error_rate > 0.3 and error_rate < 0.5:
                remaining_to_open = 0.5 - error_rate
                alerts.append({
                    "agent": agent,
                    "type": "circuit_breaker_warning",
                    "severity": "warning",
                    "message": f"Agent '{agent}' error rate at {error_rate*100:.1f}%. Circuit will open at 50%.",
                    "current_error_rate": round(error_rate, 4),
                    "threshold": 0.5,
                    "margin_until_open": round(remaining_to_open, 4)
                })
        
        return alerts
    
    def record_cache_hit(self, agent: str):
        self.redis.incr(f"metrics:{agent}:cache_hits")
    
    def record_cache_miss(self, agent: str):
        self.redis.incr(f"metrics:{agent}:cache_misses")
