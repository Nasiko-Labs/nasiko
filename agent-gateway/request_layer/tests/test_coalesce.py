"""Unit tests for the L4 request coalescer."""

import pytest

from request_layer.src import coalesce


@pytest.mark.asyncio
async def test_first_caller_becomes_leader(fake_redis) -> None:
    async with coalesce.acquire_leader(fake_redis, "translator", "abc123", ttl_seconds=30) as is_leader:
        assert is_leader is True


@pytest.mark.asyncio
async def test_second_caller_is_follower(fake_redis) -> None:
    async with coalesce.acquire_leader(fake_redis, "translator", "abc123", ttl_seconds=30) as is_leader_1:
        async with coalesce.acquire_leader(fake_redis, "translator", "abc123", ttl_seconds=30) as is_leader_2:
            assert is_leader_1 is True
            assert is_leader_2 is False


@pytest.mark.asyncio
async def test_leader_releases_lock_on_exit(fake_redis) -> None:
    async with coalesce.acquire_leader(fake_redis, "translator", "abc123", ttl_seconds=30):
        pass

    async with coalesce.acquire_leader(fake_redis, "translator", "abc123", ttl_seconds=30) as is_leader:
        assert is_leader is True


@pytest.mark.asyncio
async def test_broadcast_delivers_to_subscriber(fake_redis) -> None:
    import asyncio

    async def follower():
        return await coalesce.wait_for_broadcast(
            fake_redis, "translator", "abc123", timeout_seconds=2
        )

    follower_task = asyncio.create_task(follower())
    # Yield the loop so the follower has a chance to subscribe.
    await asyncio.sleep(0.05)
    sent = await coalesce.broadcast(fake_redis, "translator", "abc123", b'{"ok": true}')
    assert sent >= 1

    received = await follower_task
    assert received == b'{"ok": true}'
