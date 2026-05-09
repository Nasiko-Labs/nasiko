from __future__ import annotations

from collections import defaultdict

from router.src.resilience.models import RuntimeSnapshot


class RuntimeStats:
    def __init__(self):
        self.cache_hits = 0
        self.cache_misses = 0
        self.cache_stores = 0
        self.rate_limit_rejections = 0
        self.agent_errors = 0
        self.queue_depths: dict[str, int] = {}
        self.current_limits: dict[str, float] = {}
        self._agent_latency_sum: dict[str, float] = defaultdict(float)
        self._agent_latency_count: dict[str, int] = defaultdict(int)
        self._queue_wait_sum: dict[str, float] = defaultdict(float)
        self._queue_wait_count: dict[str, int] = defaultdict(int)

    def record_cache_hit(self) -> None:
        self.cache_hits += 1

    def record_cache_miss(self) -> None:
        self.cache_misses += 1

    def record_cache_store(self) -> None:
        self.cache_stores += 1

    def record_rate_limit_rejection(self) -> None:
        self.rate_limit_rejections += 1

    def record_agent_error(self, agent_id: str) -> None:
        self.agent_errors += 1
        self.set_queue_depth(agent_id, self.queue_depths.get(agent_id, 0))

    def set_queue_depth(self, agent_id: str, depth: int) -> None:
        self.queue_depths[agent_id] = max(0, depth)

    def set_current_limit(self, agent_id: str, limit: float) -> None:
        self.current_limits[agent_id] = limit

    def record_agent_latency(self, agent_id: str, latency_seconds: float) -> None:
        self._agent_latency_sum[agent_id] += max(0.0, latency_seconds)
        self._agent_latency_count[agent_id] += 1

    def record_queue_wait(self, agent_id: str, wait_seconds: float) -> None:
        self._queue_wait_sum[agent_id] += max(0.0, wait_seconds)
        self._queue_wait_count[agent_id] += 1

    def snapshot(self) -> RuntimeSnapshot:
        latency_average = {
            agent_id: self._agent_latency_sum[agent_id] / count
            for agent_id, count in self._agent_latency_count.items()
            if count
        }
        queue_wait_average = {
            agent_id: self._queue_wait_sum[agent_id] / count
            for agent_id, count in self._queue_wait_count.items()
            if count
        }
        total_cache = self.cache_hits + self.cache_misses
        return RuntimeSnapshot(
            cache_hits=self.cache_hits,
            cache_misses=self.cache_misses,
            cache_hit_ratio=self.cache_hits / total_cache if total_cache else 0.0,
            cache_stores=self.cache_stores,
            rate_limit_rejections=self.rate_limit_rejections,
            agent_errors=self.agent_errors,
            queue_depths=dict(self.queue_depths),
            current_limits=dict(self.current_limits),
            agent_latency_count=dict(self._agent_latency_count),
            agent_latency_average_seconds=latency_average,
            queue_wait_count=dict(self._queue_wait_count),
            queue_wait_average_seconds=queue_wait_average,
        )

    def prometheus_text(self) -> str:
        lines = [
            "# TYPE gateway_cache_hits_total counter",
            f"gateway_cache_hits_total {self.cache_hits}",
            "# TYPE gateway_cache_misses_total counter",
            f"gateway_cache_misses_total {self.cache_misses}",
            "# TYPE gateway_cache_hit_ratio gauge",
            f"gateway_cache_hit_ratio {self.snapshot().cache_hit_ratio}",
            "# TYPE gateway_cache_stores_total counter",
            f"gateway_cache_stores_total {self.cache_stores}",
            "# TYPE gateway_rate_limit_rejections_total counter",
            f"gateway_rate_limit_rejections_total {self.rate_limit_rejections}",
            "# TYPE gateway_agent_errors_total counter",
            f"gateway_agent_errors_total {self.agent_errors}",
            "# TYPE gateway_queue_depth gauge",
        ]
        for agent_id, depth in sorted(self.queue_depths.items()):
            lines.append(f'gateway_queue_depth{{agent_id="{agent_id}"}} {depth}')

        lines.append("# TYPE gateway_adaptive_limit_current gauge")
        for agent_id, limit in sorted(self.current_limits.items()):
            lines.append(
                f'gateway_adaptive_limit_current{{agent_id="{agent_id}"}} {limit}'
            )

        lines.append("# TYPE gateway_agent_latency_seconds counter")
        for agent_id, count in sorted(self._agent_latency_count.items()):
            total = self._agent_latency_sum[agent_id]
            lines.append(
                f'gateway_agent_latency_seconds_count{{agent_id="{agent_id}"}} {count}'
            )
            lines.append(
                f'gateway_agent_latency_seconds_sum{{agent_id="{agent_id}"}} {total}'
            )

        lines.append("# TYPE gateway_queue_wait_seconds counter")
        for agent_id, count in sorted(self._queue_wait_count.items()):
            total = self._queue_wait_sum[agent_id]
            lines.append(
                f'gateway_queue_wait_seconds_count{{agent_id="{agent_id}"}} {count}'
            )
            lines.append(
                f'gateway_queue_wait_seconds_sum{{agent_id="{agent_id}"}} {total}'
            )

        return "\n".join(lines) + "\n"
