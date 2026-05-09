"""
Centralized in-memory observability service for the AI middleware.

This module owns all counters, latency summaries, per-agent metrics, and system
health evaluation. Routes and workers call semantic helper methods instead of
mutating counters directly. Future Prometheus/Grafana/Phoenix integration can
export the structured snapshots produced here without changing request flow.
"""

from dataclasses import dataclass, field
from datetime import datetime
from threading import RLock
from typing import Any, Dict, List, Optional


HEALTH_HEALTHY = "healthy"
HEALTH_WARNING = "warning"
HEALTH_OVERLOADED = "overloaded"


@dataclass
class AgentMetrics:
    """Per-agent counters and latency tracking."""

    request_count: int = 0
    cache_hits: int = 0
    rate_limited_count: int = 0
    processed_count: int = 0
    total_processing_time: float = 0.0

    @property
    def average_processing_time(self) -> float:
        """Average processing time for completed agent work."""
        if self.processed_count == 0:
            return 0.0
        return self.total_processing_time / self.processed_count

    def to_dict(self) -> Dict[str, Any]:
        """Return a stable JSON-friendly per-agent snapshot."""
        return {
            "request_count": self.request_count,
            "cache_hits": self.cache_hits,
            "rate_limited_count": self.rate_limited_count,
            "average_processing_time": self.average_processing_time,
        }


@dataclass
class MetricsSnapshot:
    """
    Raw observability counters.

    These counters are intentionally simple: they make traffic, cache
    effectiveness, overload protection, queue stabilization, and latency
    visible during demos.
    """

    total_requests: int = 0
    processed_requests: int = 0
    active_requests: int = 0
    queued_requests: int = 0
    processed_from_queue: int = 0
    rate_limited_requests: int = 0

    cache_hits: int = 0
    cache_misses: int = 0

    current_queue_size: int = 0
    max_queue_size: int = 0
    queue_overflow_count: int = 0
    total_queue_wait_time: float = 0.0
    queue_wait_samples: int = 0

    total_processing_time: float = 0.0
    fastest_response_time: Optional[float] = None
    slowest_response_time: Optional[float] = None

    agents: Dict[str, AgentMetrics] = field(default_factory=dict)
    timestamp: str = field(default_factory=lambda: datetime.utcnow().isoformat())


class StatsCollector:
    """
    Central metrics manager for traffic, cache, queue, performance, and agents.

    The service is intentionally framework-agnostic and in-memory. Request
    handlers call semantic helper methods; endpoints consume structured
    snapshots. That boundary keeps observability isolated and makes a future
    Prometheus/Grafana exporter a thin adapter instead of a rewrite.
    """

    def __init__(
        self,
        *,
        warning_queue_ratio: float = 0.50,
        overloaded_queue_ratio: float = 0.90,
        active_warning_threshold: int = 10,
        active_overloaded_threshold: int = 25,
    ) -> None:
        # Health thresholds are deliberately configurable constructor arguments
        # so demos can tune sensitivity without changing endpoint code.
        self.metrics = MetricsSnapshot()
        self.warning_queue_ratio = warning_queue_ratio
        self.overloaded_queue_ratio = overloaded_queue_ratio
        self.active_warning_threshold = active_warning_threshold
        self.active_overloaded_threshold = active_overloaded_threshold
        # Metrics are in-memory and shared by routes plus the background queue
        # worker. The lock keeps snapshots consistent without introducing an
        # external metrics store.
        self._lock = RLock()

    def increment_metric(
        self,
        category: str,
        metric_name: str,
        amount: int | float = 1,
    ) -> None:
        """
        Generic metric increment helper for small future counters.

        Categories are accepted for readability and future exporter mapping;
        the in-memory implementation stores raw counters on MetricsSnapshot.
        """
        del category
        with self._lock:
            if not hasattr(self.metrics, metric_name):
                raise KeyError(f"Unknown metric '{metric_name}'")

            current_value = getattr(self.metrics, metric_name)
            if current_value is None:
                current_value = 0
            setattr(self.metrics, metric_name, current_value + amount)

    def record_request_started(self, agent_name: str) -> None:
        """Track a newly accepted API request and active in-flight work."""
        with self._lock:
            self.metrics.total_requests += 1
            self.metrics.active_requests += 1
            self.update_agent_metrics(agent_name, request_count=1)

    def record_request_finished(self) -> None:
        """Reduce active request count after route or worker processing exits."""
        with self._lock:
            self.metrics.active_requests = max(self.metrics.active_requests - 1, 0)

    def record_cache_hit(self, agent_name: Optional[str] = None) -> None:
        """Track cache effectiveness globally and per agent."""
        with self._lock:
            self.metrics.cache_hits += 1
            if agent_name:
                self.update_agent_metrics(agent_name, cache_hits=1)

    def record_cache_miss(self) -> None:
        """Track cache misses, making repeated-request demos visible."""
        with self._lock:
            self.metrics.cache_misses += 1

    def record_rate_limited(self, agent_name: Optional[str] = None) -> None:
        """Track limiter pressure globally and per agent."""
        with self._lock:
            self.metrics.rate_limited_requests += 1
            if agent_name:
                self.update_agent_metrics(agent_name, rate_limited_count=1)

    def record_queue_admission(
        self,
        *,
        queue_size: int,
        max_queue_size: int,
        estimated_wait_time: float = 0.0,
    ) -> None:
        """Track requests buffered by the async queue."""
        with self._lock:
            self.metrics.queued_requests += 1
            self.set_queue_status(queue_size, max_queue_size)
            self.update_queue_wait_time(estimated_wait_time)

    def record_queue_processed(self) -> None:
        """Track queued work completed by the background worker."""
        with self._lock:
            self.metrics.processed_from_queue += 1

    def record_queue_overflow(
        self,
        *,
        queue_size: Optional[int] = None,
        max_queue_size: Optional[int] = None,
    ) -> None:
        """Track requests that could not be buffered because the queue was full."""
        with self._lock:
            self.metrics.queue_overflow_count += 1
            if queue_size is not None and max_queue_size is not None:
                self.set_queue_status(queue_size, max_queue_size)

    def set_queue_status(self, queue_size: int, max_queue_size: int) -> None:
        """Update live queue depth gauges."""
        with self._lock:
            self.metrics.current_queue_size = max(queue_size, 0)
            self.metrics.max_queue_size = max(max_queue_size, 0)

    def update_queue_wait_time(self, wait_time: float) -> None:
        """Add a queue wait estimate/sample for average wait visibility."""
        with self._lock:
            if wait_time <= 0:
                return
            self.metrics.total_queue_wait_time += wait_time
            self.metrics.queue_wait_samples += 1

    def update_processing_time(
        self,
        processing_time: float,
        agent_name: Optional[str] = None,
    ) -> None:
        """Update latency totals and fastest/slowest response gauges."""
        with self._lock:
            self.metrics.total_processing_time += processing_time

            if (
                self.metrics.fastest_response_time is None
                or processing_time < self.metrics.fastest_response_time
            ):
                self.metrics.fastest_response_time = processing_time

            if (
                self.metrics.slowest_response_time is None
                or processing_time > self.metrics.slowest_response_time
            ):
                self.metrics.slowest_response_time = processing_time

            if agent_name:
                self.update_agent_metrics(
                    agent_name,
                    processed_count=1,
                    processing_time=processing_time,
                )

    def record_processing_complete(
        self,
        processing_time: float,
        agent_name: Optional[str] = None,
    ) -> None:
        """Track completed AI processing and its latency."""
        with self._lock:
            self.metrics.processed_requests += 1
            self.update_processing_time(processing_time, agent_name)

    def update_agent_metrics(
        self,
        agent_name: str,
        *,
        request_count: int = 0,
        cache_hits: int = 0,
        rate_limited_count: int = 0,
        processed_count: int = 0,
        processing_time: float = 0.0,
    ) -> None:
        """Update per-agent operational counters."""
        with self._lock:
            normalized_agent = self._normalize_agent(agent_name)
            agent_metrics = self.metrics.agents.setdefault(
                normalized_agent,
                AgentMetrics(),
            )

            agent_metrics.request_count += request_count
            agent_metrics.cache_hits += cache_hits
            agent_metrics.rate_limited_count += rate_limited_count
            agent_metrics.processed_count += processed_count
            agent_metrics.total_processing_time += processing_time

    def calculate_cache_hit_ratio(self) -> float:
        """Return cache hit ratio as a percentage."""
        with self._lock:
            total_cache_requests = self.metrics.cache_hits + self.metrics.cache_misses
            if total_cache_requests == 0:
                return 0.0
            return (self.metrics.cache_hits / total_cache_requests) * 100

    def calculate_average_response_time(self) -> float:
        """Return average processing latency for completed AI work."""
        with self._lock:
            if self.metrics.processed_requests == 0:
                return 0.0
            return self.metrics.total_processing_time / self.metrics.processed_requests

    def calculate_average_queue_wait_time(self) -> float:
        """Return average queue wait estimate/sample."""
        with self._lock:
            if self.metrics.queue_wait_samples == 0:
                return 0.0
            return self.metrics.total_queue_wait_time / self.metrics.queue_wait_samples

    def get_system_health(self) -> Dict[str, Any]:
        """
        Evaluate health from traffic, queue pressure, active work, and cache use.

        States:
        - healthy: low pressure and no material bottlenecks
        - warning: queue or active work is building
        - overloaded: queue is near full or active work is too high
        """
        with self._lock:
            warnings: List[str] = []
            bottlenecks: List[str] = []
            recommendations: List[str] = []

            # Health is intentionally derived from observable resilience
            # signals rather than external infrastructure probes.
            queue_ratio = self._queue_utilization_ratio()
            cache_hit_ratio = self.calculate_cache_hit_ratio()

            health_status = HEALTH_HEALTHY

            if self.metrics.active_requests >= self.active_overloaded_threshold:
                health_status = HEALTH_OVERLOADED
                bottlenecks.append("active_requests")
                warnings.append("Active request count is above overload threshold.")
                recommendations.append("Let the queue drain before sending more traffic.")
            elif self.metrics.active_requests >= self.active_warning_threshold:
                health_status = HEALTH_WARNING
                bottlenecks.append("active_requests")
                warnings.append("Active request count is elevated.")
                recommendations.append("Watch active request depth during the spike.")

            if queue_ratio >= self.overloaded_queue_ratio:
                health_status = HEALTH_OVERLOADED
                bottlenecks.append("queue")
                warnings.append("Queue utilization is near capacity.")
                recommendations.append("Reduce request rate or raise max_queue_size for demos.")
            elif queue_ratio >= self.warning_queue_ratio and health_status != HEALTH_OVERLOADED:
                health_status = HEALTH_WARNING
                bottlenecks.append("queue")
                warnings.append("Queue is absorbing a traffic spike.")
                recommendations.append("Monitor processed_from_queue until the queue stabilizes.")

            if self.metrics.queue_overflow_count > 0:
                health_status = HEALTH_OVERLOADED
                bottlenecks.append("queue_overflow")
                warnings.append("Some requests could not be queued because the buffer was full.")
                recommendations.append("Increase queue capacity or reduce burst size.")

            if cache_hit_ratio >= 60 and health_status == HEALTH_HEALTHY:
                recommendations.append("Cache is reducing repeated agent work effectively.")
            elif (
                self.metrics.cache_misses > self.metrics.cache_hits
                and self.metrics.total_requests > 3
            ):
                recommendations.append(
                    "Repeat identical requests to demonstrate cache acceleration."
                )

            if not warnings:
                warnings.append("No immediate resilience warnings.")
            if not bottlenecks:
                bottlenecks.append("none")
            if not recommendations:
                recommendations.append("System is stable; continue monitoring traffic spikes.")

            return {
                "health_status": health_status,
                "warnings": warnings,
                "bottlenecks": bottlenecks,
                "recommendations": recommendations,
                "signals": {
                    "queue_utilization_percent": queue_ratio * 100,
                    "active_requests": self.metrics.active_requests,
                    "cache_hit_ratio": cache_hit_ratio,
                    "queue_overflow_count": self.metrics.queue_overflow_count,
                },
                "timestamp": datetime.utcnow().isoformat(),
            }

    def get_metrics(self) -> Dict[str, Any]:
        """Return the complete structured observability snapshot."""
        with self._lock:
            self.metrics.timestamp = datetime.utcnow().isoformat()
            return {
                "traffic": self._traffic_metrics(),
                "cache": self._cache_metrics(),
                "queue": self._queue_metrics(),
                "performance": self._performance_metrics(),
                "agents": self._agents_metrics(),
                "health": self.get_system_health(),
                "timestamp": self.metrics.timestamp,
            }

    def get_agents_status(self) -> Dict[str, Any]:
        """Return per-agent operational stats for agent-focused monitoring."""
        with self._lock:
            return {
                "agents": self._agents_metrics(),
                "total_agents_observed": len(self.metrics.agents),
                "timestamp": datetime.utcnow().isoformat(),
            }

    def get_metrics_summary(self) -> Dict[str, Any]:
        """Return a compact dashboard-friendly metrics summary."""
        with self._lock:
            health = self.get_system_health()
            return {
                "health_status": health["health_status"],
                "total_requests": self.metrics.total_requests,
                "active_requests": self.metrics.active_requests,
                "current_queue_size": self.metrics.current_queue_size,
                "queued_requests": self.metrics.queued_requests,
                "processed_from_queue": self.metrics.processed_from_queue,
                "rate_limited_requests": self.metrics.rate_limited_requests,
                "cache_hit_ratio": self.calculate_cache_hit_ratio(),
                "average_response_time": self.calculate_average_response_time(),
                "queue_overflow_count": self.metrics.queue_overflow_count,
                "warnings": health["warnings"],
                "timestamp": datetime.utcnow().isoformat(),
            }

    def reset_metrics(self) -> None:
        """Safely reset metrics while leaving cache, limiter, and queue state intact."""
        with self._lock:
            self.metrics = MetricsSnapshot()

    # ------------------------------------------------------------------
    # Compatibility wrappers used by earlier phases.
    # ------------------------------------------------------------------

    def increment_cache_hits(self, agent_name: Optional[str] = None) -> None:
        """Compatibility wrapper for older cache-hit call sites."""
        self.record_cache_hit(agent_name)

    def increment_cache_misses(self) -> None:
        """Compatibility wrapper for older cache-miss call sites."""
        self.record_cache_miss()

    def increment_processed_requests(self) -> None:
        """Compatibility wrapper for older processed-request counters."""
        self.increment_metric("traffic", "processed_requests")

    def increment_rate_limited_requests(self, agent_name: Optional[str] = None) -> None:
        """Compatibility wrapper for older limiter-pressure counters."""
        self.record_rate_limited(agent_name)

    def increment_per_agent_request_count(self, agent_name: str) -> None:
        """Compatibility wrapper for older per-agent request counters."""
        with self._lock:
            self.metrics.total_requests += 1
            self.update_agent_metrics(agent_name, request_count=1)

    def increment_queued_requests(self) -> None:
        """Compatibility wrapper for older queued-request counters."""
        self.increment_metric("traffic", "queued_requests")

    def decrement_queued_requests(self) -> None:
        """Compatibility wrapper that adjusts the live queue-size gauge."""
        with self._lock:
            self.metrics.current_queue_size = max(self.metrics.current_queue_size - 1, 0)

    def increment_processed_from_queue(self) -> None:
        """Compatibility wrapper for older queue completion counters."""
        self.record_queue_processed()

    def increment_queue_overflow_count(self) -> None:
        """Compatibility wrapper for older queue overflow counters."""
        self.record_queue_overflow()

    def set_current_queue_size(self, queue_size: int) -> None:
        """Compatibility wrapper for updating the live queue-size gauge."""
        with self._lock:
            self.metrics.current_queue_size = max(queue_size, 0)

    def increment_active_requests(self) -> None:
        """Compatibility wrapper for tracking active worker/request pressure."""
        self.increment_metric("traffic", "active_requests")

    def decrement_active_requests(self) -> None:
        """Compatibility wrapper for clearing active worker/request pressure."""
        self.record_request_finished()

    def add_processing_time(
        self,
        processing_time: float,
        agent_name: Optional[str] = None,
    ) -> None:
        """Compatibility wrapper for latency aggregation."""
        self.update_processing_time(processing_time, agent_name)

    def get_average_response_time(self) -> float:
        """Compatibility wrapper for average response time calculation."""
        return self.calculate_average_response_time()

    def get_cache_hit_ratio(self) -> float:
        """Compatibility wrapper for cache hit ratio calculation."""
        return self.calculate_cache_hit_ratio()

    # ------------------------------------------------------------------
    # Internal snapshot builders.
    # ------------------------------------------------------------------

    def _traffic_metrics(self) -> Dict[str, Any]:
        """Build the traffic section of the structured metrics payload."""
        return {
            "total_requests": self.metrics.total_requests,
            "processed_requests": self.metrics.processed_requests,
            "active_requests": self.metrics.active_requests,
            "queued_requests": self.metrics.queued_requests,
            "processed_from_queue": self.metrics.processed_from_queue,
            "rate_limited_requests": self.metrics.rate_limited_requests,
        }

    def _cache_metrics(self) -> Dict[str, Any]:
        """Build the cache section of the structured metrics payload."""
        return {
            "cache_hits": self.metrics.cache_hits,
            "cache_misses": self.metrics.cache_misses,
            "cache_hit_ratio": self.calculate_cache_hit_ratio(),
        }

    def _queue_metrics(self) -> Dict[str, Any]:
        """Build the queue section of the structured metrics payload."""
        return {
            "current_queue_size": self.metrics.current_queue_size,
            "max_queue_size": self.metrics.max_queue_size,
            "queue_overflow_count": self.metrics.queue_overflow_count,
            "average_queue_wait_time": self.calculate_average_queue_wait_time(),
        }

    def _performance_metrics(self) -> Dict[str, Any]:
        """Build the latency/performance section of the metrics payload."""
        return {
            "average_response_time": self.calculate_average_response_time(),
            "fastest_response_time": self.metrics.fastest_response_time or 0.0,
            "slowest_response_time": self.metrics.slowest_response_time or 0.0,
            "total_processing_time": self.metrics.total_processing_time,
        }

    def _agents_metrics(self) -> Dict[str, Any]:
        """Build a stable per-agent metrics mapping sorted by agent name."""
        return {
            agent_name: agent_metrics.to_dict()
            for agent_name, agent_metrics in sorted(self.metrics.agents.items())
        }

    def _queue_utilization_ratio(self) -> float:
        """Return queue utilization as a 0.0-1.0 ratio for health checks."""
        if self.metrics.max_queue_size <= 0:
            return 0.0
        return self.metrics.current_queue_size / self.metrics.max_queue_size

    @staticmethod
    def _normalize_agent(agent_name: str) -> str:
        """Normalize agent identifiers for consistent per-agent metrics."""
        return agent_name.strip().lower()


# Global observability service instance.
stats_collector = StatsCollector()
