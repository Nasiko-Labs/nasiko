from router.src.resilience.limiter import AdaptiveRateLimiter
from router.src.resilience.models import LimitConfig


def test_limiter_is_independent_per_agent():
    limiter = AdaptiveRateLimiter(LimitConfig(base_rps=1, burst=1))

    assert limiter.check("agent-a", queue_depth=0).action == "allow"
    assert limiter.check("agent-a", queue_depth=0).action == "queue"
    assert limiter.check("agent-b", queue_depth=0).action == "allow"


def test_limiter_rejects_when_queue_is_full():
    limiter = AdaptiveRateLimiter(LimitConfig(base_rps=1, burst=1, max_queue_depth=1))

    assert limiter.check("agent-a", queue_depth=0).action == "allow"
    decision = limiter.check("agent-a", queue_depth=1)

    assert decision.action == "reject"
    assert decision.retry_after_seconds > 0


def test_limiter_tightens_after_latency_and_error_pressure():
    limiter = AdaptiveRateLimiter(
        LimitConfig(base_rps=10, min_rps=1, burst=10, target_latency_seconds=1)
    )

    healthy = limiter.effective_rps("agent-a", queue_depth=0)
    limiter.record_result("agent-a", latency_seconds=4, success=False)
    pressured = limiter.effective_rps("agent-a", queue_depth=10)

    assert pressured < healthy
    assert pressured >= 1


def test_limiter_config_can_be_updated_per_agent():
    limiter = AdaptiveRateLimiter(LimitConfig(base_rps=5, burst=5))

    limiter.update_config("agent-a", base_rps=2, burst=2)

    assert limiter.get_config("agent-a").base_rps == 2
    assert limiter.get_config("agent-a").burst == 2
