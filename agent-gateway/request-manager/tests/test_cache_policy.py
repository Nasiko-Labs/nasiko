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


def decide(json_body: dict, *, headers: dict[str, str] | None = None):
    return CachePolicy().decide(
        agent_id="agent-a2a-demo",
        target=make_target(),
        limits=make_limits(),
        headers=headers or {"X-Subject-ID": "subject-123"},
        json_body=json_body,
    )


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
    decision = decide(
        text_message_body(),
        headers={"Cache-Control": "max-age=0, No-Cache", "X-Subject-ID": "subject-123"},
    )

    assert decision.cacheable is False
    assert decision.reason == "cache-control-no-cache"


def test_cache_control_no_store_bypasses_cache():
    decision = decide(
        text_message_body(),
        headers={"Cache-Control": "max-age=0, NO-STORE", "X-Subject-ID": "subject-123"},
    )

    assert decision.cacheable is False
    assert decision.reason == "cache-control-no-store"


def test_cache_disabled_bypasses_cache():
    decision = CachePolicy().decide(
        agent_id="agent-a2a-demo",
        target=make_target(),
        limits=make_limits(cache_enabled=False),
        headers={"X-Subject-ID": "subject-123"},
        json_body=text_message_body(),
    )

    assert decision.cacheable is False
    assert decision.reason == "agent-cache-disabled"


def test_unsupported_method_bypasses_cache():
    body = text_message_body()
    body["method"] = "message/stream"

    decision = decide(body)

    assert decision.cacheable is False
    assert decision.reason == "unsupported-method"


@pytest.mark.parametrize("jsonrpc", [None, "1.0"])
def test_missing_or_wrong_jsonrpc_bypasses_cache(jsonrpc):
    body = text_message_body()
    if jsonrpc is None:
        body.pop("jsonrpc")
    else:
        body["jsonrpc"] = jsonrpc

    decision = decide(body)

    assert decision.cacheable is False
    assert decision.reason == "unsupported-jsonrpc"


@pytest.mark.parametrize(
    "body",
    [
        {"jsonrpc": "2.0", "method": "message/send", "params": "bad"},
        {"jsonrpc": "2.0", "method": "message/send", "params": {"message": "bad"}},
        {"jsonrpc": "2.0", "method": "message/send", "params": {"message": {"parts": "bad"}}},
        {"jsonrpc": "2.0", "method": "message/send", "params": {"message": {"parts": []}}},
    ],
)
def test_malformed_message_shapes_bypass_without_exceptions(body):
    decision = decide(body)

    assert decision.cacheable is False
    assert decision.reason == "invalid-message-shape"


@pytest.mark.parametrize("headers", [{}, {"X-Subject-ID": ""}, {"X-Subject-ID": "   "}])
def test_missing_or_empty_subject_scope_bypasses_cache(headers):
    decision = decide(text_message_body(), headers=headers)

    assert decision.cacheable is False
    assert decision.reason == "missing-subject-scope"


def test_non_text_parts_bypass_cache():
    body = text_message_body()
    body["params"]["message"]["parts"].append(
        {"kind": "file", "file": {"name": "notes.txt"}},
    )

    decision = CachePolicy().decide(
        agent_id="agent-a2a-demo",
        target=make_target(),
        limits=make_limits(),
        headers={"X-Subject-ID": "subject-123"},
        json_body=body,
    )

    assert decision.cacheable is False
    assert decision.reason == "non-text-part"


def test_different_target_revision_produces_different_cache_key():
    policy = CachePolicy()

    first = policy.decide(
        agent_id="agent-a2a-demo",
        target=make_target("rev-1"),
        limits=make_limits(),
        headers={"X-Subject-ID": "subject-123"},
        json_body=text_message_body(),
    )
    second = policy.decide(
        agent_id="agent-a2a-demo",
        target=make_target("rev-2"),
        limits=make_limits(),
        headers={"X-Subject-ID": "subject-123"},
        json_body=text_message_body(),
    )

    assert first.cache_key != second.cache_key


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


@pytest.mark.asyncio
async def test_redis_response_cache_get_handles_raw_bytes(fake_redis):
    cache = RedisResponseCache(fake_redis)
    response = CachedResponse(
        status_code=200,
        media_type="application/json",
        body=b'{"ok":true}',
        headers={"content-type": "application/json"},
    )

    await cache.set("cache-key", response, ttl_seconds=30)
    key = "request-manager:cache:cache-key"
    fake_redis.values[key] = fake_redis.values[key].encode("utf-8")

    assert await cache.get("cache-key") == response


@pytest.mark.asyncio
async def test_redis_response_cache_get_missing_returns_none(fake_redis):
    assert await RedisResponseCache(fake_redis).get("missing-key") is None


class RaisingRedis:
    async def get(self, key: str):
        raise RuntimeError("redis unavailable")

    async def set(self, key: str, value: str, ex: int | None = None):
        raise RuntimeError("redis unavailable")


@pytest.mark.asyncio
async def test_redis_response_cache_get_exception_returns_none():
    assert await RedisResponseCache(RaisingRedis()).get("cache-key") is None


@pytest.mark.asyncio
async def test_redis_response_cache_set_exception_is_swallowed():
    response = CachedResponse(status_code=200, media_type="text/plain", body=b"ok")

    await RedisResponseCache(RaisingRedis()).set("cache-key", response, ttl_seconds=30)


@pytest.mark.asyncio
async def test_redis_response_cache_clear_fake_redis_deletes_cache_keys_only(fake_redis):
    await fake_redis.set("request-manager:cache:key-1", "value")
    await fake_redis.set("request-manager:cache:key-2", "value")
    await fake_redis.set("request-manager:not-cache:key-3", "value")

    removed = await RedisResponseCache(fake_redis).clear()

    assert removed == 2
    assert "request-manager:cache:key-1" not in fake_redis.values
    assert "request-manager:cache:key-2" not in fake_redis.values
    assert fake_redis.values["request-manager:not-cache:key-3"] == "value"


@pytest.mark.asyncio
async def test_redis_response_cache_clear_with_agent_id_returns_zero(fake_redis):
    await fake_redis.set("request-manager:cache:key-1", "value")

    removed = await RedisResponseCache(fake_redis).clear(agent_id="agent-a2a-demo")

    assert removed == 0
    assert fake_redis.values["request-manager:cache:key-1"] == "value"


class ScanIterRedis:
    def __init__(self) -> None:
        self.values = {
            "request-manager:cache:key-1": "value",
            "request-manager:cache:key-2": "value",
            "request-manager:not-cache:key-3": "value",
        }

    async def scan_iter(self, match: str):
        prefix = match.removesuffix("*")
        for key in list(self.values):
            if key.startswith(prefix):
                yield key

    async def delete(self, *keys: str) -> int:
        removed = 0
        for key in keys:
            if key in self.values:
                removed += 1
                self.values.pop(key)
        return removed


@pytest.mark.asyncio
async def test_redis_response_cache_clear_scan_iter_deletes_cache_keys_only():
    redis = ScanIterRedis()

    removed = await RedisResponseCache(redis).clear()

    assert removed == 2
    assert "request-manager:cache:key-1" not in redis.values
    assert "request-manager:cache:key-2" not in redis.values
    assert redis.values["request-manager:not-cache:key-3"] == "value"
