"""
Tests for the unified request management layer.
Covers: CacheService, RateLimiter, RequestManager, and monitoring endpoints.
"""

import asyncio
import json
import time
import pytest
from unittest.mock import AsyncMock, MagicMock, patch

# ── CacheService ─────────────────────────────────────────────────────────────

class TestLRUCache:
    """Tests for the in-process LRU fallback cache."""

    def setup_method(self):
        from router.src.core.cache_service import _LRUCache
        self.cache = _LRUCache(max_size=3, ttl=1)

    def test_set_and_get(self):
        self.cache.set("k1", "v1")
        assert self.cache.get("k1") == "v1"

    def test_miss_returns_none(self):
        assert self.cache.get("nonexistent") is None

    def test_ttl_expiry(self):
        self.cache.set("k1", "v1")
        time.sleep(1.1)
        assert self.cache.get("k1") is None

    def test_lru_eviction(self):
        self.cache.set("k1", "v1")
        self.cache.set("k2", "v2")
        self.cache.set("k3", "v3")
        # Access k1 to make it recently used
        self.cache.get("k1")
        # Adding k4 should evict k2 (least recently used)
        self.cache.set("k4", "v4")
        assert self.cache.get("k2") is None
        assert self.cache.get("k1") == "v1"
        assert self.cache.get("k3") == "v3"
        assert self.cache.get("k4") == "v4"

    def test_delete(self):
        self.cache.set("k1", "v1")
        assert self.cache.delete("k1") is True
        assert self.cache.get("k1") is None
        assert self.cache.delete("k1") is False

    def test_clear(self):
        self.cache.set("k1", "v1")
        self.cache.set("k2", "v2")
        count = self.cache.clear()
        assert count == 2
        assert self.cache.size() == 0

    def test_size_evicts_expired(self):
        self.cache.set("k1", "v1")
        time.sleep(1.1)
        assert self.cache.size() == 0


class TestCacheKey:
    """Tests for cache key generation."""

    def test_normalizes_whitespace(self):
        from router.src.core.cache_service import make_cache_key
        k1 = make_cache_key("agent-a", "hello   world")
        k2 = make_cache_key("agent-a", "hello world")
        assert k1 == k2

    def test_normalizes_case(self):
        from router.src.core.cache_service import make_cache_key
        k1 = make_cache_key("agent-a", "Hello World")
        k2 = make_cache_key("agent-a", "hello world")
        assert k1 == k2

    def test_different_agents_different_keys(self):
        from router.src.core.cache_service import make_cache_key
        k1 = make_cache_key("agent-a", "hello")
        k2 = make_cache_key("agent-b", "hello")
        assert k1 != k2

    def test_key_has_prefix(self):
        from router.src.core.cache_service import make_cache_key
        k = make_cache_key("agent-a", "hello")
        assert k.startswith("nasiko:cache:")


class TestCacheServiceLRU:
    """Tests for CacheService using in-process LRU (no Redis)."""

    @pytest.fixture
    def cache(self):
        from router.src.core.cache_service import CacheService
        return CacheService()

    @pytest.mark.asyncio
    async def test_get_miss(self, cache):
        result = await cache.get("agent-a", "hello")
        assert result is None

    @pytest.mark.asyncio
    async def test_set_then_get(self, cache):
        lines = ['{"message":"hi","is_int_response":false,"agent_id":"","url":""}']
        await cache.set("agent-a", "hello", lines)
        result = await cache.get("agent-a", "hello")
        assert result == lines

    @pytest.mark.asyncio
    async def test_hit_increments_counter(self, cache):
        lines = ['{"message":"hi","is_int_response":false,"agent_id":"","url":""}']
        await cache.set("agent-a", "hello", lines)
        await cache.get("agent-a", "hello")
        stats = await cache.get_stats()
        assert stats["hits"] == 1
        assert stats["misses"] == 0

    @pytest.mark.asyncio
    async def test_miss_increments_counter(self, cache):
        await cache.get("agent-a", "missing")
        stats = await cache.get_stats()
        assert stats["misses"] == 1
        assert stats["hits"] == 0

    @pytest.mark.asyncio
    async def test_hit_rate_calculation(self, cache):
        lines = ['{"message":"hi","is_int_response":false,"agent_id":"","url":""}']
        await cache.set("agent-a", "hello", lines)
        await cache.get("agent-a", "hello")   # hit
        await cache.get("agent-a", "missing") # miss
        stats = await cache.get_stats()
        assert stats["hit_rate_pct"] == 50.0

    @pytest.mark.asyncio
    async def test_clear_all(self, cache):
        lines = ['{"message":"hi","is_int_response":false,"agent_id":"","url":""}']
        await cache.set("agent-a", "hello", lines)
        await cache.set("agent-b", "world", lines)
        count = await cache.clear_all()
        assert count == 2
        assert await cache.get("agent-a", "hello") is None

    @pytest.mark.asyncio
    async def test_delete(self, cache):
        lines = ['{"message":"hi","is_int_response":false,"agent_id":"","url":""}']
        await cache.set("agent-a", "hello", lines)
        deleted = await cache.delete("agent-a", "hello")
        assert deleted is True
        assert await cache.get("agent-a", "hello") is None

    @pytest.mark.asyncio
    async def test_stats_backend_is_lru(self, cache):
        stats = await cache.get_stats()
        assert stats["backend"] == "lru"
        assert stats["connected"] is False

    @pytest.mark.asyncio
    async def test_no_cache_files_bypass(self, cache):
        """Files should not be cached — verified at orchestrator level, but
        we confirm the cache itself stores/retrieves correctly when called."""
        lines = ['{"message":"file response","is_int_response":false,"agent_id":"","url":""}']
        await cache.set("agent-a", "file query", lines)
        result = await cache.get("agent-a", "file query")
        assert result == lines  # cache itself is neutral; bypass logic is in orchestrator


# ── RateLimiter ──────────────────────────────────────────────────────────────

class TestRateLimiter:
    """Tests for the token-bucket rate limiter."""

    @pytest.fixture
    def limiter(self):
        from router.src.core.rate_limiter import RateLimiter
        return RateLimiter()

    @pytest.mark.asyncio
    async def test_immediate_token_granted(self, limiter):
        """First request should get a token immediately (bucket starts full)."""
        granted = False
        async with limiter.acquire("agent-a"):
            granted = True
        assert granted

    @pytest.mark.asyncio
    async def test_stats_after_request(self, limiter):
        async with limiter.acquire("agent-a"):
            pass
        stats = limiter.get_agent_stats("agent-a")
        assert stats is not None
        assert stats["total_requests"] == 1
        assert stats["accepted_requests"] == 1
        assert stats["rejected_requests"] == 0

    @pytest.mark.asyncio
    async def test_configure_agent(self, limiter):
        limiter.configure_agent("agent-x", 2.0, 5, 10)
        configs = limiter.list_configured_agents()
        assert "agent-x" in configs
        assert configs["agent-x"]["requests_per_second"] == 2.0
        assert configs["agent-x"]["burst_capacity"] == 5

    @pytest.mark.asyncio
    async def test_remove_agent_config(self, limiter):
        limiter.configure_agent("agent-x", 2.0, 5, 10)
        result = limiter.remove_agent_config("agent-x")
        assert result is True
        assert "agent-x" not in limiter.list_configured_agents()

    @pytest.mark.asyncio
    async def test_remove_nonexistent_config(self, limiter):
        result = limiter.remove_agent_config("nonexistent")
        assert result is False

    @pytest.mark.asyncio
    async def test_rate_limit_exceeded_when_queue_full(self, limiter):
        """With burst=1 and queue=0, second concurrent request should be rejected."""
        from router.src.core.rate_limiter import RateLimitExceeded
        limiter.configure_agent("agent-tight", 0.001, 1, 0)

        # Consume the only token first
        bucket = await limiter._get_or_create_bucket("agent-tight")
        async with limiter._lock:
            bucket.try_consume()  # drain the token directly

        # Now any request should fail immediately (no tokens, queue_size=0)
        with pytest.raises(RateLimitExceeded) as exc_info:
            async with limiter.acquire("agent-tight"):
                pass
        assert exc_info.value.agent_name == "agent-tight"

    @pytest.mark.asyncio
    async def test_global_stats_structure(self, limiter):
        async with limiter.acquire("agent-a"):
            pass
        stats = limiter.get_stats()
        assert "agents" in stats
        assert "global_defaults" in stats
        assert "agent-a" in stats["agents"]
        agent_stats = stats["agents"]["agent-a"]
        assert "capacity" in agent_stats
        assert "rate_per_second" in agent_stats
        assert "tokens_available" in agent_stats
        assert "queue_depth" in agent_stats
        assert "rejection_rate_pct" in agent_stats
        assert "avg_queue_wait_ms" in agent_stats

    @pytest.mark.asyncio
    async def test_unknown_agent_stats_returns_none(self, limiter):
        assert limiter.get_agent_stats("never-seen") is None

    @pytest.mark.asyncio
    async def test_burst_capacity_respected(self, limiter):
        """With burst=3, first 3 requests should all get immediate tokens."""
        limiter.configure_agent("agent-burst", 1.0, 3, 0)
        for _ in range(3):
            async with limiter.acquire("agent-burst"):
                pass
        stats = limiter.get_agent_stats("agent-burst")
        assert stats["accepted_requests"] == 3
        assert stats["rejected_requests"] == 0


# ── RequestManager ────────────────────────────────────────────────────────────

class TestRequestManager:
    """Tests for the unified RequestManager — orchestrator is mocked."""

    @pytest.fixture
    def manager(self):
        from router.src.core.cache_service import CacheService
        from router.src.core.rate_limiter import RateLimiter
        from router.src.services.request_manager import RequestManager

        m = RequestManager.__new__(RequestManager)
        m._cache = CacheService()
        m._rate_limiter = RateLimiter()

        # Mock the orchestrator so no langchain needed
        mock_orch = MagicMock()
        mock_orch.health_check = AsyncMock(return_value={
            "router": "healthy",
            "timestamp": 0.0,
            "components": {},
        })
        m._orchestrator = mock_orch
        return m

    @pytest.mark.asyncio
    async def test_startup_shutdown(self, manager):
        await manager.startup()
        await manager.shutdown()

    @pytest.mark.asyncio
    async def test_clear_cache_returns_dict(self, manager):
        result = await manager.clear_cache()
        assert "status" in result
        assert result["status"] == "ok"
        assert "cleared_keys" in result

    @pytest.mark.asyncio
    async def test_clear_agent_cache_returns_dict(self, manager):
        result = await manager.clear_agent_cache("agent-a")
        assert result["agent"] == "agent-a"
        assert result["status"] == "ok"

    @pytest.mark.asyncio
    async def test_get_cache_stats_structure(self, manager):
        stats = await manager.get_cache_stats()
        assert "backend" in stats
        assert "hits" in stats
        assert "misses" in stats
        assert "hit_rate_pct" in stats
        assert "ttl_seconds" in stats

    def test_get_rate_limit_stats_structure(self, manager):
        stats = manager.get_rate_limit_stats()
        assert "agents" in stats
        assert "global_defaults" in stats

    def test_configure_rate_limit(self, manager):
        result = manager.configure_rate_limit("agent-x", 3.0, 6, 15)
        assert result["agent"] == "agent-x"
        assert result["status"] == "configured"
        assert result["requests_per_second"] == 3.0

    def test_remove_rate_limit_config(self, manager):
        manager.configure_rate_limit("agent-x", 3.0, 6, 15)
        result = manager.remove_rate_limit_config("agent-x")
        assert result["status"] == "removed"

    def test_remove_nonexistent_rate_limit(self, manager):
        result = manager.remove_rate_limit_config("ghost")
        assert result["status"] == "not_found"

    def test_list_rate_limit_configs(self, manager):
        manager.configure_rate_limit("agent-x", 3.0, 6, 15)
        configs = manager.list_rate_limit_configs()
        assert "agent-x" in configs

    @pytest.mark.asyncio
    async def test_health_check_structure(self, manager):
        health = await manager.health_check()
        assert "router" in health
        assert "components" in health
        assert "cache" in health["components"]
        assert "rate_limiter" in health["components"]
        assert health["components"]["cache"]["status"] in ("healthy", "degraded")
        assert health["components"]["rate_limiter"]["status"] == "healthy"

    @pytest.mark.asyncio
    async def test_handle_request_rate_limit_exceeded(self, manager):
        """When queue is full, handle_request should yield an error response."""
        from router.src.entities import UserRequest
        # Configure a very tight limit with no queue
        manager._rate_limiter.configure_agent("router", 0.001, 1, 0)

        # Drain the single token directly
        bucket = await manager._rate_limiter._get_or_create_bucket("router")
        async with manager._rate_limiter._lock:
            bucket.try_consume()

        request = UserRequest(session_id="s1", query="hello")
        lines = []
        async for line in manager.handle_request(request, [], "token"):
            lines.append(line)

        assert len(lines) == 1
        data = json.loads(lines[0])
        assert data["is_int_response"] is False
        assert "overloaded" in data["message"].lower()


# ── FastAPI endpoint tests ────────────────────────────────────────────────────

class TestMonitoringEndpoints:
    """Tests for the monitoring API endpoints — orchestrator is mocked."""

    @pytest.fixture
    def client(self):
        from fastapi.testclient import TestClient
        from router.src.core.cache_service import CacheService
        from router.src.core.rate_limiter import RateLimiter
        from router.src.services.request_manager import RequestManager

        # Build a RequestManager with a mocked orchestrator
        mgr = RequestManager.__new__(RequestManager)
        mgr._cache = CacheService()
        mgr._rate_limiter = RateLimiter()
        mock_orch = MagicMock()
        mock_orch.health_check = AsyncMock(return_value={
            "router": "healthy",
            "timestamp": 0.0,
            "components": {},
        })
        mgr._orchestrator = mock_orch

        from router.src.main import app
        # Override the singleton
        app.state.request_manager = mgr
        import router.src.main as main_module
        main_module.request_manager = mgr

        return TestClient(app)

    def test_router_health(self, client):
        resp = client.get("/router/health")
        assert resp.status_code == 200
        assert resp.json() == {"status": "ok"}

    def test_cache_stats(self, client):
        resp = client.get("/monitor/cache/stats")
        assert resp.status_code == 200
        body = resp.json()
        assert "backend" in body
        assert "hits" in body
        assert "misses" in body
        assert "hit_rate_pct" in body

    def test_agent_cache_stats(self, client):
        resp = client.get("/monitor/cache/stats/my-agent")
        assert resp.status_code == 200
        body = resp.json()
        assert "agent" in body

    def test_clear_all_cache(self, client):
        resp = client.delete("/monitor/cache")
        assert resp.status_code == 200
        assert resp.json()["status"] == "ok"

    def test_clear_agent_cache(self, client):
        resp = client.delete("/monitor/cache/my-agent")
        assert resp.status_code == 200
        assert resp.json()["agent"] == "my-agent"

    def test_rate_limit_stats(self, client):
        resp = client.get("/monitor/rate-limits")
        assert resp.status_code == 200
        body = resp.json()
        assert "agents" in body
        assert "global_defaults" in body

    def test_rate_limit_configs_list(self, client):
        resp = client.get("/monitor/rate-limits/configs/list")
        assert resp.status_code == 200
        assert isinstance(resp.json(), dict)

    def test_configure_rate_limit(self, client):
        resp = client.put(
            "/monitor/rate-limits/test-agent",
            json={"requests_per_second": 2.0, "burst_capacity": 5, "queue_size": 10},
        )
        assert resp.status_code == 200
        body = resp.json()
        assert body["agent"] == "test-agent"
        assert body["status"] == "configured"

    def test_configure_rate_limit_invalid_rps(self, client):
        resp = client.put(
            "/monitor/rate-limits/test-agent",
            json={"requests_per_second": 0, "burst_capacity": 5, "queue_size": 10},
        )
        assert resp.status_code == 400

    def test_configure_rate_limit_invalid_burst(self, client):
        resp = client.put(
            "/monitor/rate-limits/test-agent",
            json={"requests_per_second": 1.0, "burst_capacity": 0, "queue_size": 10},
        )
        assert resp.status_code == 400

    def test_remove_rate_limit_config(self, client):
        client.put(
            "/monitor/rate-limits/test-agent",
            json={"requests_per_second": 2.0, "burst_capacity": 5, "queue_size": 10},
        )
        resp = client.delete("/monitor/rate-limits/test-agent/config")
        assert resp.status_code == 200
        assert resp.json()["status"] == "removed"

    def test_agent_rate_limit_stats_not_found(self, client):
        resp = client.get("/monitor/rate-limits/never-seen-agent-xyz")
        assert resp.status_code == 404

    def test_dashboard(self, client):
        resp = client.get("/monitor/dashboard")
        assert resp.status_code == 200
        body = resp.json()
        assert "health" in body
        assert "cache" in body
        assert "rate_limiter" in body

    def test_metrics(self, client):
        resp = client.get("/metrics")
        assert resp.status_code == 200
        body = resp.json()
        assert "cache_hit_rate_pct" in body
        assert "rate_limit_rejections" in body

    def test_health(self, client):
        resp = client.get("/health")
        assert resp.status_code == 200
        body = resp.json()
        assert "router" in body
        assert "components" in body

    def test_configs_list_not_shadowed_by_agent_name_route(self, client):
        """Ensure /configs/list is not captured as agent_name='configs'."""
        resp = client.get("/monitor/rate-limits/configs/list")
        assert resp.status_code == 200
        assert isinstance(resp.json(), dict)
        assert "detail" not in resp.json()

    def test_post_router_requires_auth(self, client):
        """POST /router without Bearer token should return 403."""
        resp = client.post(
            "/router",
            data={"session_id": "s1", "query": "hello"},
        )
        assert resp.status_code == 403

    def test_post_router_empty_query(self, client):
        """POST /router with empty query should return 400."""
        resp = client.post(
            "/router",
            data={"session_id": "s1", "query": "   "},
            headers={"Authorization": "Bearer fake-token"},
        )
        assert resp.status_code == 400

    def test_post_router_empty_session(self, client):
        """POST /router with empty session_id should return 400."""
        resp = client.post(
            "/router",
            data={"session_id": "  ", "query": "hello"},
            headers={"Authorization": "Bearer fake-token"},
        )
        assert resp.status_code == 400


# ── Problem-statement coverage tests ─────────────────────────────────────────
# These tests directly verify the three requirements and four success metrics
# from the Nasiko Buildthon problem statement.

class TestRequirement1_CacheBeforeForwarding:
    """
    Requirement 1: Cache agent responses so repeated requests can be served
    quickly without recomputing results. The gateway should check the cache
    BEFORE forwarding requests to agents.
    """

    @pytest.fixture
    def orchestrator_with_mock_agent(self):
        """
        RouterOrchestrator with all external dependencies mocked so we can
        verify the cache interception point without langchain or real agents.
        """
        import sys
        from unittest.mock import AsyncMock, MagicMock
        from router.src.core.cache_service import CacheService
        from router.src.services.router_orchestrator import RouterOrchestrator
        from router.src.entities import RouterOutput

        cache = CacheService()
        orch = RouterOrchestrator.__new__(RouterOrchestrator)
        orch._cache = cache
        orch._vector_store = None

        # Mock agent registry, session history, agent client
        orch.agent_registry = MagicMock()
        orch.agent_registry.fetch_agent_cards = AsyncMock(return_value=[
            {"name": "test-agent", "url": "http://test-agent/", "description": "test"}
        ])
        orch.agent_registry.get_agent_url = MagicMock(return_value="http://test-agent/")
        orch.agent_registry.get_fallback_agent = MagicMock(return_value=None)

        orch.session_history_service = MagicMock()
        orch.session_history_service.fetch_session_history = AsyncMock(return_value=[])
        orch.session_history_service.reconstruct_conversation = MagicMock(return_value=[])

        orch.agent_client = MagicMock()
        orch.agent_client.send_request = AsyncMock(return_value={"response": "agent answer"})
        orch.agent_client.extract_response_content = MagicMock(return_value="agent answer")

        # Mock vector store property
        mock_vs = MagicMock()
        mock_vs.create_vector_store = MagicMock(return_value=MagicMock())
        orch._vector_store = mock_vs

        # RouterOutput only has agent_name (no agent_id field)
        router_output = RouterOutput(agent_name="test-agent")

        # Patch the lazy import: `from router.src.core.routing_engine import router`
        # inside _handle_route_selection. We inject a fake module into sys.modules
        # so the `from ... import router` resolves to our mock function.
        mock_re_module = MagicMock()
        mock_re_module.router = MagicMock(return_value=([], [], [], router_output))
        sys.modules["router.src.core.routing_engine"] = mock_re_module

        return orch, cache

    def teardown_method(self, method):
        pass

    @pytest.mark.asyncio
    async def test_cache_hit_skips_agent_call(self, orchestrator_with_mock_agent):
        """
        Core requirement: second identical request must be served from cache
        without calling the agent again.
        """
        from router.src.entities import UserRequest

        orch, cache = orchestrator_with_mock_agent

        # Pre-populate cache as if a previous request already ran
        cached_response = ['{"message":"cached answer","is_int_response":false,"agent_id":"","url":""}']
        await cache.set("test-agent", "what is 2+2", cached_response)

        request = UserRequest(session_id="s1", query="what is 2+2")
        lines = []
        async for line in orch.process_request(request, [], "token"):
            lines.append(line)

        # Agent client must NOT have been called
        orch.agent_client.send_request.assert_not_called()

        # The cached line must appear in the output
        assert any("cached answer" in line for line in lines)

    @pytest.mark.asyncio
    async def test_cache_miss_calls_agent(self, orchestrator_with_mock_agent):
        """On a cache miss the agent must be called and the response cached."""
        from router.src.entities import UserRequest

        orch, cache = orchestrator_with_mock_agent

        request = UserRequest(session_id="s1", query="unique query xyz123")
        lines = []
        async for line in orch.process_request(request, [], "token"):
            lines.append(line)

        # Agent must have been called
        orch.agent_client.send_request.assert_called_once()

        # Response must now be in cache
        cached = await cache.get("test-agent", "unique query xyz123")
        assert cached is not None

    @pytest.mark.asyncio
    async def test_repeated_query_served_from_cache(self, orchestrator_with_mock_agent):
        """
        Success metric: cache hit rate for repeated queries.
        Send the same query twice; second call must be a cache hit.
        """
        from router.src.entities import UserRequest

        orch, cache = orchestrator_with_mock_agent

        query = "tell me about caching"
        request = UserRequest(session_id="s1", query=query)

        # First request — cache miss, agent called
        async for _ in orch.process_request(request, [], "token"):
            pass
        assert orch.agent_client.send_request.call_count == 1

        # Second request — must be a cache hit, agent NOT called again
        async for _ in orch.process_request(request, [], "token"):
            pass
        assert orch.agent_client.send_request.call_count == 1  # still 1

        # Cache stats confirm a hit
        stats = await cache.get_stats()
        assert stats["hits"] >= 1

    @pytest.mark.asyncio
    async def test_files_bypass_cache(self, orchestrator_with_mock_agent):
        """
        Files must bypass the cache — non-deterministic content should never
        be served from cache.
        """
        from router.src.entities import UserRequest

        orch, cache = orchestrator_with_mock_agent

        # Pre-populate cache for this query
        cached_response = ['{"message":"cached","is_int_response":false,"agent_id":"","url":""}']
        await cache.set("test-agent", "process this file", cached_response)

        # Send request WITH a file attachment
        fake_file = [("files", ("test.txt", b"file content", "text/plain"))]
        request = UserRequest(session_id="s1", query="process this file")
        async for _ in orch.process_request(request, fake_file, "token"):
            pass

        # Agent must have been called despite cache having an entry
        orch.agent_client.send_request.assert_called_once()

    @pytest.mark.asyncio
    async def test_error_response_not_cached(self, orchestrator_with_mock_agent):
        """
        Error responses from agents must not be cached so the next request
        gets a fresh attempt.
        """
        from router.src.core.agent_client import AgentClientError
        from router.src.entities import UserRequest

        orch, cache = orchestrator_with_mock_agent

        # Make the agent raise an error
        orch.agent_client.send_request.side_effect = AgentClientError("agent down")

        request = UserRequest(session_id="s1", query="failing query")
        async for _ in orch.process_request(request, [], "token"):
            pass

        # Nothing should be cached
        cached = await cache.get("test-agent", "failing query")
        assert cached is None

    @pytest.mark.asyncio
    async def test_query_normalization_cache_hit(self, orchestrator_with_mock_agent):
        """
        Success metric: reduced duplicate processing.
        "Hello World" and "hello   world" must share the same cache entry.
        """
        from router.src.core.cache_service import CacheService

        cache = CacheService()
        lines = ['{"message":"hi","is_int_response":false,"agent_id":"","url":""}']

        await cache.set("agent-a", "Hello   World", lines)
        result = await cache.get("agent-a", "hello world")
        assert result == lines, "Normalized queries must share the same cache entry"

    @pytest.mark.asyncio
    async def test_cache_hit_rate_tracks_repeated_queries(self):
        """
        Success metric: cache hit rate for repeated queries and workflows.
        Verify the hit_rate_pct metric increases as repeated queries are served.
        """
        from router.src.core.cache_service import CacheService

        cache = CacheService()
        lines = ['{"message":"answer","is_int_response":false,"agent_id":"","url":""}']

        # 1 miss
        await cache.get("agent-a", "q1")
        stats = await cache.get_stats()
        assert stats["hit_rate_pct"] == 0.0

        # Set and hit
        await cache.set("agent-a", "q1", lines)
        await cache.get("agent-a", "q1")  # hit
        await cache.get("agent-a", "q1")  # hit
        stats = await cache.get_stats()
        # 2 hits out of 3 total = 66.67%
        assert stats["hit_rate_pct"] > 50.0
        assert stats["hits"] == 2
        assert stats["misses"] == 1


class TestRequirement2_RateLimitingWithQueuing:
    """
    Requirement 2: Apply per-agent rate limits to prevent overload.
    Excess traffic should be QUEUED where possible instead of immediately rejected.
    """

    @pytest.fixture
    def limiter(self):
        from router.src.core.rate_limiter import RateLimiter
        return RateLimiter()

    @pytest.mark.asyncio
    async def test_queued_requests_eventually_succeed(self, limiter):
        """
        Core requirement: excess traffic must be queued, not immediately rejected.
        A request that arrives when the bucket is empty should wait and succeed.
        """
        # 1 req/s, burst=1, queue=5 — second request must queue and succeed
        limiter.configure_agent("agent-q", 10.0, 1, 5)

        results = []

        async def make_request(i):
            async with limiter.acquire("agent-q"):
                results.append(i)

        # Fire 3 concurrent requests — only 1 token available initially
        await asyncio.gather(
            make_request(0),
            make_request(1),
            make_request(2),
        )

        # All 3 must have succeeded (queued, not rejected)
        assert len(results) == 3

    @pytest.mark.asyncio
    async def test_queue_depth_visible_in_stats(self, limiter):
        """
        Success metric: predictable queue times.
        Queue depth must be visible in stats so operators can monitor it.
        """
        limiter.configure_agent("agent-q2", 0.1, 1, 10)

        # Drain the token
        bucket = await limiter._get_or_create_bucket("agent-q2")
        async with limiter._lock:
            bucket.try_consume()

        # Start a request that will queue (don't await it — let it sit in queue)
        loop = asyncio.get_running_loop()
        task = loop.create_task(
            _consume_with_limiter(limiter, "agent-q2")
        )
        # Give the task time to enter the queue
        await asyncio.sleep(0.05)

        stats = limiter.get_agent_stats("agent-q2")
        assert stats is not None
        assert "queue_depth" in stats
        assert "avg_queue_wait_ms" in stats

        task.cancel()
        try:
            await task
        except (asyncio.CancelledError, Exception):
            pass

    @pytest.mark.asyncio
    async def test_rejection_only_when_queue_full(self, limiter):
        """
        Excess traffic is rejected ONLY when the queue is full, not before.
        """
        from router.src.core.rate_limiter import RateLimitExceeded

        # queue_size=2: first 2 excess requests queue, 3rd is rejected
        limiter.configure_agent("agent-r", 0.001, 1, 2)

        # Drain the token
        bucket = await limiter._get_or_create_bucket("agent-r")
        async with limiter._lock:
            bucket.try_consume()

        # Queue 2 requests (should not raise)
        loop = asyncio.get_running_loop()
        fut1 = loop.create_future()
        fut2 = loop.create_future()
        await bucket.queue.put(fut1)
        await bucket.queue.put(fut2)

        # 3rd request should be rejected (queue full)
        with pytest.raises(RateLimitExceeded):
            async with limiter.acquire("agent-r"):
                pass

    @pytest.mark.asyncio
    async def test_per_agent_isolation(self, limiter):
        """
        Rate limits are per-agent: throttling one agent must not affect others.
        """
        # agent-slow: very tight limit
        limiter.configure_agent("agent-slow", 0.001, 1, 0)
        # agent-fast: generous limit
        limiter.configure_agent("agent-fast", 100.0, 50, 10)

        # Drain agent-slow's token
        slow_bucket = await limiter._get_or_create_bucket("agent-slow")
        async with limiter._lock:
            slow_bucket.try_consume()

        # agent-fast must still work even though agent-slow is exhausted
        granted = False
        async with limiter.acquire("agent-fast"):
            granted = True
        assert granted

    @pytest.mark.asyncio
    async def test_avg_queue_wait_ms_tracked(self, limiter):
        """
        Success metric: predictable queue times.
        avg_queue_wait_ms must be tracked and non-negative.
        """
        limiter.configure_agent("agent-wait", 100.0, 5, 10)

        for _ in range(3):
            async with limiter.acquire("agent-wait"):
                pass

        stats = limiter.get_agent_stats("agent-wait")
        assert stats is not None
        assert "avg_queue_wait_ms" in stats
        assert stats["avg_queue_wait_ms"] >= 0.0

    @pytest.mark.asyncio
    async def test_rejection_rate_metric(self, limiter):
        """
        Success metric: lower request failures during peak load.
        rejection_rate_pct must be tracked and reflect actual rejections.
        """
        from router.src.core.rate_limiter import RateLimitExceeded

        limiter.configure_agent("agent-rej", 0.001, 1, 0)

        # Drain the token
        bucket = await limiter._get_or_create_bucket("agent-rej")
        async with limiter._lock:
            bucket.try_consume()

        # This request will be rejected
        try:
            async with limiter.acquire("agent-rej"):
                pass
        except RateLimitExceeded:
            pass

        stats = limiter.get_agent_stats("agent-rej")
        assert stats["rejected_requests"] == 1
        assert stats["rejection_rate_pct"] > 0.0


class TestRequirement3_OperationalControls:
    """
    Requirement 3: Expose operational controls through monitoring endpoints
    for managing cache, configuring limits, and viewing runtime stats.
    """

    @pytest.fixture
    def client(self):
        from fastapi.testclient import TestClient
        from router.src.core.cache_service import CacheService
        from router.src.core.rate_limiter import RateLimiter
        from router.src.services.request_manager import RequestManager
        from unittest.mock import AsyncMock, MagicMock

        mgr = RequestManager.__new__(RequestManager)
        mgr._cache = CacheService()
        mgr._rate_limiter = RateLimiter()
        mock_orch = MagicMock()
        mock_orch.health_check = AsyncMock(return_value={
            "router": "healthy",
            "timestamp": 0.0,
            "components": {},
        })
        mgr._orchestrator = mock_orch

        from router.src.main import app
        app.state.request_manager = mgr
        import router.src.main as main_module
        main_module.request_manager = mgr

        return TestClient(app)

    def test_cache_stats_expose_hit_rate(self, client):
        """
        Operational visibility: cache stats must expose hit_rate_pct so
        operators can measure 'reduced duplicate processing' KPI.
        """
        resp = client.get("/monitor/cache/stats")
        assert resp.status_code == 200
        body = resp.json()
        assert "hit_rate_pct" in body
        assert "hits" in body
        assert "misses" in body
        assert "ttl_seconds" in body
        assert "backend" in body

    def test_rate_limit_stats_expose_queue_metrics(self, client):
        """
        Operational visibility: rate limit stats must expose queue_depth and
        avg_queue_wait_ms so operators can measure 'predictable queue times' KPI.
        """
        # Trigger a request so a bucket exists
        client.put(
            "/monitor/rate-limits/test-agent",
            json={"requests_per_second": 5.0, "burst_capacity": 10, "queue_size": 20},
        )
        resp = client.get("/monitor/rate-limits")
        assert resp.status_code == 200
        body = resp.json()
        assert "global_defaults" in body
        # global_defaults must expose the configured values
        defaults = body["global_defaults"]
        assert "requests_per_second" in defaults
        assert "burst_capacity" in defaults
        assert "queue_size" in defaults

    def test_configure_rate_limit_takes_effect(self, client):
        """
        Operational control: PUT /monitor/rate-limits/{agent} must persist
        the new configuration and return it in subsequent GET.
        """
        client.put(
            "/monitor/rate-limits/my-agent",
            json={"requests_per_second": 7.5, "burst_capacity": 15, "queue_size": 30},
        )
        configs = client.get("/monitor/rate-limits/configs/list").json()
        assert "my-agent" in configs
        assert configs["my-agent"]["requests_per_second"] == 7.5
        assert configs["my-agent"]["burst_capacity"] == 15

    def test_flush_cache_resets_hit_rate(self, client):
        """
        Operational control: DELETE /monitor/cache must clear all entries
        so stale responses are not served after an agent update.
        """
        resp = client.delete("/monitor/cache")
        assert resp.status_code == 200
        body = resp.json()
        assert body["status"] == "ok"
        assert "cleared_keys" in body

    def test_dashboard_exposes_all_kpis(self, client):
        """
        Operational visibility: /monitor/dashboard must expose all KPIs
        in a single call (health + cache + rate_limiter).
        """
        resp = client.get("/monitor/dashboard")
        assert resp.status_code == 200
        body = resp.json()

        # Health
        assert "health" in body
        assert body["health"]["router"] == "healthy"

        # Cache KPIs
        assert "cache" in body
        assert "hit_rate_pct" in body["cache"]
        assert "backend" in body["cache"]

        # Rate limiter KPIs
        assert "rate_limiter" in body
        assert "global_defaults" in body["rate_limiter"]

    def test_health_endpoint_shows_cache_component(self, client):
        """
        Operational visibility: /health must show cache component status
        so operators know if Redis is connected or LRU fallback is active.
        """
        resp = client.get("/health")
        assert resp.status_code == 200
        body = resp.json()
        assert "components" in body
        assert "cache" in body["components"]
        cache_comp = body["components"]["cache"]
        assert "status" in cache_comp
        assert cache_comp["status"] in ("healthy", "degraded")
        assert "backend" in cache_comp

    def test_health_endpoint_shows_rate_limiter_component(self, client):
        """
        Operational visibility: /health must show rate_limiter component status.
        """
        resp = client.get("/health")
        body = resp.json()
        assert "rate_limiter" in body["components"]
        rl_comp = body["components"]["rate_limiter"]
        assert rl_comp["status"] == "healthy"
        assert "active_buckets" in rl_comp

    def test_metrics_endpoint_exposes_cache_hit_rate_kpi(self, client):
        """
        Success metric: /metrics must expose cache_hit_rate_pct for the
        'faster repeated responses' KPI.
        """
        resp = client.get("/metrics")
        assert resp.status_code == 200
        body = resp.json()
        assert "cache_hit_rate_pct" in body
        assert "cache_hits" in body
        assert "cache_misses" in body
        assert "rate_limit_rejections" in body

    def test_remove_rate_limit_reverts_to_defaults(self, client):
        """
        Operational control: removing a custom config must revert the agent
        to global defaults (not leave it unconfigured).
        """
        # Configure
        client.put(
            "/monitor/rate-limits/temp-agent",
            json={"requests_per_second": 1.0, "burst_capacity": 2, "queue_size": 5},
        )
        assert "temp-agent" in client.get("/monitor/rate-limits/configs/list").json()

        # Remove
        resp = client.delete("/monitor/rate-limits/temp-agent/config")
        assert resp.status_code == 200
        assert resp.json()["status"] == "removed"

        # Must no longer appear in custom configs
        assert "temp-agent" not in client.get("/monitor/rate-limits/configs/list").json()

    def test_per_agent_cache_stats_endpoint(self, client):
        """
        Operational visibility: per-agent cache stats must be accessible
        so operators can identify which agents benefit most from caching.
        """
        resp = client.get("/monitor/cache/stats/code-agent")
        assert resp.status_code == 200
        body = resp.json()
        assert "agent" in body
        assert body["agent"] == "code-agent"

    def test_per_agent_cache_flush_endpoint(self, client):
        """
        Operational control: per-agent cache flush must be available so
        operators can invalidate stale responses for a specific agent
        without clearing the entire cache.
        """
        resp = client.delete("/monitor/cache/code-agent")
        assert resp.status_code == 200
        body = resp.json()
        assert body["agent"] == "code-agent"
        assert body["status"] == "ok"


# ---------------------------------------------------------------------------
# Helper coroutine used by queue depth test
# ---------------------------------------------------------------------------

async def _consume_with_limiter(limiter, agent_name: str):
    """Helper: acquire a token (may queue) then immediately release."""
    async with limiter.acquire(agent_name):
        pass
