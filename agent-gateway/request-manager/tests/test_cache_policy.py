import pytest

from request_manager.cache import CachePolicy, RedisResponseCache, normalize_text
from request_manager.models import AgentLimits, AgentTarget, CachedResponse


def make_limits(cache_enabled: bool = True) -> AgentLimits:
    return AgentLimits(
        cache_ttl_seconds=60,
        max_concurrency=2,
        sustained_rps=5.0,
        burst_capacity=10,
        max_queue_depth=20,
        max_queue_wait_ms=10_000,
        cache_enabled=cache_enabled,
    )


def make_target(target_revision: str = "rev-1") -> AgentTarget:
    return AgentTarget(
        agent_id="agent-a2a-demo",
        public_path="/agents/agent-a2a-demo",
        upstream_url="http://agent-a2a-demo:5000",
        target_revision=target_revision,
        source="docker",
        namespace="docker-agents",
        updated_at=123.4,
    )


def text_message_body(jsonrpc_id: int | str = 1) -> dict:
    return {
        "jsonrpc": "2.0",
        "id": jsonrpc_id,
        "method": "message/send",
        "params": {
            "message": {
                "parts": [
                    {"kind": "text", "text": "  Hello   WORLD  "},
                    {"kind": "text", "text": "Second\tLine"},
                ],
            },
        },
    }


def test_normalize_text_trims_lowercases_and_collapses_whitespace():
    assert normalize_text("  Hello   WORLD  ") == "hello world"


def test_message_send_text_only_body_is_cacheable_with_stable_fingerprint():
    decision = CachePolicy().decide(
        agent_id="agent-a2a-demo",
        target=make_target("rev-2"),
        limits=make_limits(),
        headers={"X-Subject-ID": "subject-123"},
        json_body=text_message_body(),
    )

    assert decision.cacheable is True
    assert decision.reason == "cacheable"
    assert decision.cache_key is not None
    assert decision.fingerprint == {
        "agent_id": "agent-a2a-demo",
        "method": "message/send",
        "scope": "subject-123",
        "target_revision": "rev-2",
        "texts": ["hello world", "second line"],
    }


def test_cache_key_ignores_json_rpc_id():
    policy = CachePolicy()

    first = policy.decide(
        agent_id="agent-a2a-demo",
        target=make_target(),
        limits=make_limits(),
        headers={"X-Subject-ID": "subject-123"},
        json_body=text_message_body(jsonrpc_id=1),
    )
    second = policy.decide(
        agent_id="agent-a2a-demo",
        target=make_target(),
        limits=make_limits(),
        headers={"X-Subject-ID": "subject-123"},
        json_body=text_message_body(jsonrpc_id="different-id"),
    )

    assert first.cacheable is True
    assert second.cacheable is True
    assert first.cache_key == second.cache_key


def test_cache_control_no_cache_bypasses_cache():
    decision = CachePolicy().decide(
        agent_id="agent-a2a-demo",
        target=make_target(),
        limits=make_limits(),
        headers={"Cache-Control": "max-age=0, no-cache"},
        json_body=text_message_body(),
    )

    assert decision.cacheable is False
    assert decision.reason == "cache-control-no-cache"


def test_non_text_parts_bypass_cache():
    body = text_message_body()
    body["params"]["message"]["parts"].append(
        {"kind": "file", "file": {"name": "notes.txt"}},
    )

    decision = CachePolicy().decide(
        agent_id="agent-a2a-demo",
        target=make_target(),
        limits=make_limits(),
        headers={},
        json_body=body,
    )

    assert decision.cacheable is False
    assert decision.reason == "non-text-part"


@pytest.mark.asyncio
async def test_redis_response_cache_round_trips_cached_response_bytes_and_headers(fake_redis):
    cache = RedisResponseCache(fake_redis)
    response = CachedResponse(
        status_code=200,
        media_type="application/json",
        body=b'{"ok":true}',
        headers={"content-type": "application/json", "x-cacheable": "yes"},
    )

    await cache.set("cache-key", response, ttl_seconds=30)
    cached = await cache.get("cache-key")

    assert cached == response
    assert fake_redis.expiry["request-manager:cache:cache-key"] == 30_000
