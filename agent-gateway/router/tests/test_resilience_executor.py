import asyncio

from router.src.entities import UserRequest
from router.src.resilience.executor import ResilientAgentExecutor
from router.src.resilience.models import CacheConfig, LimitConfig, ResilienceError


def test_executor_serves_repeated_request_from_cache():
    async def run():
        calls = 0
        executor = ResilientAgentExecutor(
            cache_config=CacheConfig(ttl_seconds=60),
            limit_config=LimitConfig(base_rps=100, burst=100),
        )
        request = UserRequest(session_id="s1", query="What is sick leave?", route=None)

        async def real_call():
            nonlocal calls
            calls += 1
            return "10 days"

        first = await executor.execute(
            agent_id="agent-a",
            request=request,
            files=[],
            token="tenant-a:user-1",
            call_agent=real_call,
        )
        second = await executor.execute(
            agent_id="agent-a",
            request=request,
            files=[],
            token="tenant-a:user-1",
            call_agent=real_call,
        )

        assert first == "10 days"
        assert second == "10 days"
        assert calls == 1

    asyncio.run(run())


def test_executor_does_not_cache_file_requests():
    async def run():
        calls = 0
        executor = ResilientAgentExecutor(
            cache_config=CacheConfig(ttl_seconds=60),
            limit_config=LimitConfig(base_rps=100, burst=100),
        )
        request = UserRequest(session_id="s1", query="Summarize file", route=None)

        async def real_call():
            nonlocal calls
            calls += 1
            return f"response-{calls}"

        assert (
            await executor.execute(
                agent_id="agent-a",
                request=request,
                files=[("files", ("a.txt", b"x", "text/plain"))],
                token="scope",
                call_agent=real_call,
            )
            == "response-1"
        )
        assert (
            await executor.execute(
                agent_id="agent-a",
                request=request,
                files=[("files", ("a.txt", b"x", "text/plain"))],
                token="scope",
                call_agent=real_call,
            )
            == "response-2"
        )

    asyncio.run(run())


def test_executor_returns_bounded_failure_when_queue_is_full():
    async def run():
        executor = ResilientAgentExecutor(
            cache_config=CacheConfig(ttl_seconds=60),
            limit_config=LimitConfig(
                base_rps=0.1, burst=1, max_queue_depth=0, max_queue_wait_seconds=0.01
            ),
        )
        request = UserRequest(session_id="s1", query="expensive", route=None)

        async def slow_call():
            await asyncio.sleep(0.05)
            return "ok"

        first = asyncio.create_task(
            executor.execute("agent-a", request, [], "scope", slow_call)
        )
        await asyncio.sleep(0)

        try:
            await executor.execute("agent-a", request, [], "different-scope", slow_call)
        except ResilienceError as exc:
            assert exc.status_code == 429
            assert exc.retry_after_seconds >= 0
        else:
            raise AssertionError("expected bounded queue failure")

        assert await first == "ok"

    asyncio.run(run())
