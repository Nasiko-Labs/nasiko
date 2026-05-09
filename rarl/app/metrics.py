import asyncio
import collections
import time
from typing import Any


class Metrics:
    def __init__(self) -> None:
        self.qps_in: collections.deque[float] = collections.deque(maxlen=120)
        self.qps_upstream: collections.deque[float] = collections.deque(maxlen=120)
        self.p95_latency_client: collections.deque[float] = collections.deque(maxlen=120)
        self.p95_latency_upstream: collections.deque[float] = collections.deque(maxlen=120)
        self.total_cache_hit_rate: collections.deque[float] = collections.deque(maxlen=120)

        self._tick_requests = 0
        self._tick_upstream = 0
        self._tick_client_latencies: list[float] = []
        self._tick_upstream_latencies: list[float] = []

        self._total_requests = 0
        self._total_cache_hits = 0
        self._total_coalesced = 0

        self.per_agent: dict[str, dict[str, Any]] = {}

    def record_request(
        self,
        agent_id: str,
        was_cache_hit: bool,
        was_coalesced: bool,
        client_latency: float,
        upstream_latency: float | None,
    ) -> None:
        self._tick_requests += 1
        self._total_requests += 1

        if was_cache_hit:
            self._total_cache_hits += 1
        if was_coalesced:
            self._total_coalesced += 1

        self._tick_client_latencies.append(client_latency)
        if upstream_latency is not None:
            self._tick_upstream += 1
            self._tick_upstream_latencies.append(upstream_latency)

        if agent_id not in self.per_agent:
            self.per_agent[agent_id] = {
                "requests": 0, "cache_hits": 0, "coalesced": 0, "upstream_calls": 0
            }
        ag = self.per_agent[agent_id]
        ag["requests"] += 1
        if was_cache_hit:
            ag["cache_hits"] += 1
        if was_coalesced:
            ag["coalesced"] += 1
        if upstream_latency is not None:
            ag["upstream_calls"] += 1

    def _percentile(self, data: list[float], pct: float) -> float:
        if not data:
            return 0.0
        s = sorted(data)
        return s[max(0, int(pct * len(s)) - 1)]

    def _tick(self) -> None:
        self.qps_in.append(float(self._tick_requests))
        self.qps_upstream.append(float(self._tick_upstream))
        self.p95_latency_client.append(
            self._percentile(self._tick_client_latencies, 0.95) * 1000
        )
        self.p95_latency_upstream.append(
            self._percentile(self._tick_upstream_latencies, 0.95) * 1000
        )
        total = self._total_requests
        self.total_cache_hit_rate.append(
            self._total_cache_hits / total if total else 0.0
        )
        self._tick_requests = 0
        self._tick_upstream = 0
        self._tick_client_latencies = []
        self._tick_upstream_latencies = []

    def start_tick_task(self) -> asyncio.Task:
        async def _run() -> None:
            while True:
                await asyncio.sleep(1)
                self._tick()

        return asyncio.create_task(_run())

    def snapshot(self) -> dict[str, Any]:
        return {
            "timestamp": time.time(),
            "total_requests": self._total_requests,
            "total_cache_hits": self._total_cache_hits,
            "hit_rate": self._total_cache_hits / self._total_requests if self._total_requests else 0.0,
            "coalesced_count": self._total_coalesced,
            "qps_in": list(self.qps_in),
            "qps_upstream": list(self.qps_upstream),
            "p95_client_ms": list(self.p95_latency_client),
            "p95_upstream_ms": list(self.p95_latency_upstream),
            "cache_hit_rate_history": list(self.total_cache_hit_rate),
            "per_agent": dict(self.per_agent),
        }
