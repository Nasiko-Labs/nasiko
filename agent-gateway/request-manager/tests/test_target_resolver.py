import pytest

from request_manager.models import AgentLimits, AgentTarget
from request_manager.settings import RequestManagerSettings
from request_manager.target_resolver import LimitResolver, TargetResolver


@pytest.mark.asyncio
async def test_resolves_agent_target_from_redis(fake_redis):
    await fake_redis.hset(
        "request-manager:targets:agent-a2a-demo",
        mapping={
            "agent_id": "agent-a2a-demo",
            "public_path": "/agents/agent-a2a-demo",
            "upstream_url": "http://agent-a2a-demo:5000",
            "target_revision": "rev-1",
            "source": "docker",
            "namespace": "docker-agents",
            "updated_at": "123.4",
        },
    )

    resolver = TargetResolver(fake_redis)
    target = await resolver.resolve("agent-a2a-demo")

    assert isinstance(target, AgentTarget)
    assert target.upstream_url == "http://agent-a2a-demo:5000"
    assert target.target_revision == "rev-1"


@pytest.mark.asyncio
async def test_returns_none_when_agent_target_is_missing(fake_redis):
    resolver = TargetResolver(fake_redis)

    assert await resolver.resolve("missing-agent") is None


@pytest.mark.asyncio
async def test_missing_agent_target_clears_stale_memory(fake_redis):
    await fake_redis.hset(
        "request-manager:targets:agent-a2a-demo",
        mapping={
            "agent_id": "agent-a2a-demo",
            "public_path": "/agents/agent-a2a-demo",
            "upstream_url": "http://agent-a2a-demo:5000",
            "target_revision": "rev-1",
            "source": "docker",
            "namespace": "docker-agents",
            "updated_at": "123.4",
        },
    )
    resolver = TargetResolver(fake_redis)
    assert await resolver.resolve("agent-a2a-demo") is not None

    await fake_redis.delete("request-manager:targets:agent-a2a-demo")

    assert await resolver.resolve("agent-a2a-demo") is None
    assert "agent-a2a-demo" not in resolver.memory


@pytest.mark.asyncio
async def test_invalid_partial_payload_falls_back_to_last_known_good_target(fake_redis):
    await fake_redis.hset(
        "request-manager:targets:agent-a2a-demo",
        mapping={
            "agent_id": "agent-a2a-demo",
            "public_path": "/agents/agent-a2a-demo",
            "upstream_url": "http://agent-a2a-demo:5000",
            "target_revision": "rev-1",
            "source": "docker",
            "namespace": "docker-agents",
            "updated_at": "123.4",
        },
    )
    resolver = TargetResolver(fake_redis)
    first_target = await resolver.resolve("agent-a2a-demo")

    await fake_redis.delete("request-manager:targets:agent-a2a-demo")
    await fake_redis.hset(
        "request-manager:targets:agent-a2a-demo",
        mapping={
            "agent_id": "agent-a2a-demo",
            "public_path": "/agents/agent-a2a-demo",
            "updated_at": "not-a-float",
        },
    )

    assert await resolver.resolve("agent-a2a-demo") == first_target


@pytest.mark.asyncio
async def test_limit_resolver_merges_redis_overrides(fake_redis):
    await fake_redis.hset(
        "request-manager:limits:agent-a2a-demo",
        mapping={"max_concurrency": "7", "cache_enabled": "false"},
    )
    settings = RequestManagerSettings(redis_url="redis://redis:6379")
    resolver = LimitResolver(fake_redis, settings)

    limits = await resolver.resolve("agent-a2a-demo")

    assert limits.max_concurrency == 7
    assert limits.cache_enabled is False
    assert limits.cache_ttl_seconds == 600


@pytest.mark.asyncio
async def test_limit_resolver_update_serializes_cache_enabled_as_string(fake_redis):
    settings = RequestManagerSettings(redis_url="redis://redis:6379")
    resolver = LimitResolver(fake_redis, settings)
    limits = AgentLimits(
        cache_ttl_seconds=30,
        max_concurrency=3,
        sustained_rps=2.5,
        burst_capacity=5,
        max_queue_depth=8,
        max_queue_wait_ms=900,
        cache_enabled=False,
    )

    await resolver.update("agent-a2a-demo", limits)

    stored = await fake_redis.hgetall("request-manager:limits:agent-a2a-demo")
    assert stored["cache_enabled"] == "false"
    assert isinstance(stored["cache_enabled"], str)
