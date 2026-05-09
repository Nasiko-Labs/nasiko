import redis
import time
from typing import Dict, Any
from enum import Enum

class CircuitState(Enum):
    CLOSED = "CLOSED"       # Normal operation
    HALF_OPEN = "HALF_OPEN" # Caution - limited traffic
    OPEN = "OPEN"           # Blocked - predictive protection

class PredictiveCircuitBreaker:
    def __init__(self, redis_host: str = "redis", redis_port: int = 6379):
        self.redis = redis.Redis(
            host=redis_host,
            port=redis_port,
            decode_responses=True,
            socket_connect_timeout=5,
            socket_timeout=5
        )
        self.half_open_threshold = 0.30  # 30% error rate
        self.open_threshold = 0.50       # 50% error rate - PREDICTIVE
        self.half_open_traffic_percent = 0.10  # 10% traffic in half-open
        self.recovery_timeout = 30  # seconds before trying half-open
        
    def _get_error_key(self, agent: str) -> str:
        return f"circuit:{agent}:errors"
    
    def _get_total_key(self, agent: str) -> str:
        return f"circuit:{agent}:total"
    
    def _get_state_key(self, agent: str) -> str:
        return f"circuit:{agent}:state"
    
    def _get_last_open_key(self, agent: str) -> str:
        return f"circuit:{agent}:last_open"
    
    def record_request(self, agent: str, success: bool) -> Dict[str, Any]:
        """Record request result and update circuit state"""
        error_key = self._get_error_key(agent)
        total_key = self._get_total_key(agent)
        state_key = self._get_state_key(agent)
        
        # Increment totals
        self.redis.incr(total_key)
        if not success:
            self.redis.incr(error_key)
        
        # Get current stats
        errors = int(self.redis.get(error_key) or 0)
        total = int(self.redis.get(total_key) or 1)
        error_rate = errors / total
        
        # Determine state
        current_state = self.redis.get(state_key) or CircuitState.CLOSED.value
        
        if current_state == CircuitState.CLOSED.value:
            if error_rate >= self.open_threshold:
                self.redis.set(state_key, CircuitState.OPEN.value)
                self.redis.set(self._get_last_open_key(agent), str(time.time()))
                return {
                    "state": CircuitState.OPEN.value,
                    "error_rate": error_rate,
                    "action": "circuit_opened_predictively",
                    "message": f"Circuit opened at {error_rate:.1%} error rate (threshold: {self.open_threshold:.0%})"
                }
            elif error_rate >= self.half_open_threshold:
                self.redis.set(state_key, CircuitState.HALF_OPEN.value)
                return {
                    "state": CircuitState.HALF_OPEN.value,
                    "error_rate": error_rate,
                    "action": "caution_mode",
                    "message": f"Circuit half-open at {error_rate:.1%} error rate"
                }
        
        elif current_state == CircuitState.HALF_OPEN.value:
            if error_rate >= self.open_threshold:
                self.redis.set(state_key, CircuitState.OPEN.value)
                self.redis.set(self._get_last_open_key(agent), str(time.time()))
                return {
                    "state": CircuitState.OPEN.value,
                    "error_rate": error_rate,
                    "action": "circuit_opened_from_half_open"
                }
            elif error_rate < self.half_open_threshold:
                self.redis.set(state_key, CircuitState.CLOSED.value)
                return {
                    "state": CircuitState.CLOSED.value,
                    "error_rate": error_rate,
                    "action": "circuit_closed",
                    "message": "Error rate dropped, circuit closed"
                }
        
        elif current_state == CircuitState.OPEN.value:
            # Check if recovery timeout passed
            last_open = float(self.redis.get(self._get_last_open_key(agent)) or 0)
            if time.time() - last_open >= self.recovery_timeout:
                self.redis.set(state_key, CircuitState.HALF_OPEN.value)
                # Reset counters for fresh evaluation
                self.redis.set(error_key, "0")
                self.redis.set(total_key, "0")
                return {
                    "state": CircuitState.HALF_OPEN.value,
                    "error_rate": 0,
                    "action": "attempting_recovery",
                    "message": f"Recovery timeout passed, trying half-open with {self.half_open_traffic_percent:.0%} traffic"
                }
        
        return {
            "state": current_state,
            "error_rate": error_rate,
            "action": "no_change"
        }
    
    def check_circuit(self, agent: str) -> Dict[str, Any]:
        """Check if request should be allowed through circuit"""
        state_key = self._get_state_key(agent)
        current_state = self.redis.get(state_key) or CircuitState.CLOSED.value
        
        errors = int(self.redis.get(self._get_error_key(agent)) or 0)
        total = int(self.redis.get(self._get_total_key(agent)) or 1)
        error_rate = errors / total
        
        if current_state == CircuitState.OPEN.value:
            return {
                "allowed": False,
                "state": CircuitState.OPEN.value,
                "error_rate": error_rate,
                "action": "reject_and_queue",
                "message": "Circuit OPEN - requests queued or served from stale cache",
                "predictive": True
            }
        
        elif current_state == CircuitState.HALF_OPEN.value:
            # Allow limited traffic (10%)
            import random
            if random.random() < self.half_open_traffic_percent:
                return {
                    "allowed": True,
                    "state": CircuitState.HALF_OPEN.value,
                    "error_rate": error_rate,
                    "action": "probe_request",
                    "message": "Circuit half-open - allowing probe request"
                }
            else:
                return {
                    "allowed": False,
                    "state": CircuitState.HALF_OPEN.value,
                    "error_rate": error_rate,
                    "action": "queue_probe",
                    "message": "Circuit half-open - queuing non-probe request"
                }
        
        return {
            "allowed": True,
            "state": CircuitState.CLOSED.value,
            "error_rate": error_rate,
            "action": "normal_operation"
        }
    
    def get_status(self, agent: str) -> Dict[str, Any]:
        errors = int(self.redis.get(self._get_error_key(agent)) or 0)
        total = int(self.redis.get(self._get_total_key(agent)) or 1)
        state = self.redis.get(self._get_state_key(agent)) or CircuitState.CLOSED.value
        last_open = self.redis.get(self._get_last_open_key(agent))
        
        return {
            "agent": agent,
            "state": state,
            "error_rate": errors / max(total, 1),
            "total_requests": total,
            "error_requests": errors,
            "last_opened": float(last_open) if last_open else None,
            "thresholds": {
                "half_open": self.half_open_threshold,
                "open": self.open_threshold,
                "recovery_timeout": self.recovery_timeout
            }
        }
    
    def manual_reset(self, agent: str) -> Dict[str, Any]:
        """Manual circuit reset (for admin operations)"""
        self.redis.set(self._get_state_key(agent), CircuitState.CLOSED.value)
        self.redis.set(self._get_error_key(agent), "0")
        self.redis.set(self._get_total_key(agent), "0")
        return {"reset": True, "agent": agent, "new_state": CircuitState.CLOSED.value}
