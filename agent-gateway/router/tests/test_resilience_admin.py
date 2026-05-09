from urllib.parse import quote

from fastapi import FastAPI
from fastapi.testclient import TestClient

from router.src.resilience.admin import build_admin_router
from router.src.resilience.executor import ResilientAgentExecutor
from router.src.resilience.models import CacheConfig, LimitConfig
from router.src.resilience.stats import RuntimeStats


def test_runtime_stats_snapshot_and_prometheus_metrics_include_kpis():
    stats = RuntimeStats()
    stats.record_cache_hit()
    stats.record_cache_miss()
    stats.record_cache_store()
    stats.set_queue_depth("agent-a", 3)
    stats.set_current_limit("agent-a", 2.5)
    stats.record_agent_latency("agent-a", 1.5)
    stats.record_queue_wait("agent-a", 0.25)
    stats.record_rate_limit_rejection()
    stats.record_agent_error("agent-a")

    snapshot = stats.snapshot()
    metrics = stats.prometheus_text()

    assert snapshot.cache_hits == 1
    assert snapshot.cache_misses == 1
    assert snapshot.cache_hit_ratio == 0.5
    assert snapshot.queue_depths["agent-a"] == 3
    assert snapshot.current_limits["agent-a"] == 2.5
    assert "gateway_cache_hits_total 1" in metrics
    assert "gateway_cache_misses_total 1" in metrics
    assert 'gateway_queue_depth{agent_id="agent-a"} 3' in metrics
    assert 'gateway_adaptive_limit_current{agent_id="agent-a"} 2.5' in metrics


def test_limit_endpoint_accepts_url_agent_ids():
    executor = ResilientAgentExecutor(
        cache_config=CacheConfig(ttl_seconds=60),
        limit_config=LimitConfig(base_rps=5, burst=5),
    )
    app = FastAPI()
    app.include_router(build_admin_router(executor))
    client = TestClient(app)

    agent_id = "http://localhost:9100/agents/agent-translator/"
    response = client.put(
        f"/admin/limits/{quote(agent_id, safe='')}",
        headers={"X-Admin-API-Key": "local-admin-key"},
        json={"base_rps": 0.5, "burst": 1, "max_queue_depth": 3},
    )

    assert response.status_code == 200
    config = executor.limiter.get_config(agent_id)
    assert config.base_rps == 0.5
    assert config.burst == 1
    assert config.max_queue_depth == 3
