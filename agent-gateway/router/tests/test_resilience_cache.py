import time

from router.src.resilience.cache import SemanticResponseCache
from router.src.resilience.models import CacheConfig, CacheLookup


def test_repeated_normalized_request_hits_cache_without_agent_recompute():
    cache = SemanticResponseCache(CacheConfig(ttl_seconds=60))
    lookup = CacheLookup(
        agent_id="compliance-agent",
        query="  What is OUR sick leave policy? ",
        auth_scope="tenant-a:user-1",
    )

    assert cache.get(lookup) is None

    cache.set(lookup, "Employees get 10 sick days.")

    repeated = CacheLookup(
        agent_id="compliance-agent",
        query="what is our sick leave policy",
        auth_scope="tenant-a:user-1",
    )
    assert cache.get(repeated) == "Employees get 10 sick days."


def test_cache_is_isolated_by_auth_scope_and_agent_identity():
    cache = SemanticResponseCache(CacheConfig(ttl_seconds=60))
    lookup = CacheLookup(
        agent_id="agent-a",
        query="show account phone",
        auth_scope="tenant-a:user-1",
    )
    cache.set(lookup, "555-0101")

    assert (
        cache.get(
            CacheLookup(
                agent_id="agent-a",
                query="show account phone",
                auth_scope="tenant-a:user-2",
            )
        )
        is None
    )
    assert (
        cache.get(
            CacheLookup(
                agent_id="agent-b",
                query="show account phone",
                auth_scope="tenant-a:user-1",
            )
        )
        is None
    )


def test_cache_entry_expires_after_ttl():
    cache = SemanticResponseCache(CacheConfig(ttl_seconds=0.01))
    lookup = CacheLookup(
        agent_id="agent-a",
        query="summarize policy",
        auth_scope="tenant-a:user-1",
    )
    cache.set(lookup, "cached")

    time.sleep(0.02)

    assert cache.get(lookup) is None


def test_file_upload_requests_are_not_cacheable():
    cache = SemanticResponseCache(CacheConfig(ttl_seconds=60))
    lookup = CacheLookup(
        agent_id="agent-a",
        query="summarize this file",
        auth_scope="tenant-a:user-1",
        has_files=True,
    )

    cache.set(lookup, "cached")

    assert cache.get(lookup) is None


def test_cache_can_clear_all_or_by_agent():
    cache = SemanticResponseCache(CacheConfig(ttl_seconds=60))
    agent_a = CacheLookup(agent_id="agent-a", query="hello", auth_scope="scope")
    agent_b = CacheLookup(agent_id="agent-b", query="hello", auth_scope="scope")
    cache.set(agent_a, "a")
    cache.set(agent_b, "b")

    assert cache.clear(agent_id="agent-a") == 1
    assert cache.get(agent_a) is None
    assert cache.get(agent_b) == "b"

    assert cache.clear() == 1
    assert cache.get(agent_b) is None
