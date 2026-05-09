import pytest

from request_manager.models import AgentTarget
from request_manager.target_resolver import TargetResolver


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


from request_manager.settings import RequestManagerSettings
from request_manager.target_resolver import LimitResolver


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
