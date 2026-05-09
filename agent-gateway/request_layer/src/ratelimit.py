"""Token-bucket rate gate and rolling cost meter."""
import logging
import math
import time
from dataclasses import dataclass

from redis.asyncio import Redis

logger = logging.getLogger(__name__)


# Token-bucket Lua script.
#
# KEYS[1] = bucket key
# ARGV[1] = capacity (max tokens)
# ARGV[2] = refill rate (tokens / second)
# ARGV[3] = now (seconds, float as string)
# ARGV[4] = cost (tokens consumed by this request)
#
# Returns:
#   {allowed (1/0), tokens_remaining, retry_after_ms}
_TOKEN_BUCKET_LUA = """
local capacity = tonumber(ARGV[1])
local refill_rate = tonumber(ARGV[2])
local now = tonumber(ARGV[3])
local cost = tonumber(ARGV[4])

local data = redis.call('HMGET', KEYS[1], 'tokens', 'last_refill')
local tokens = tonumber(data[1])
local last_refill = tonumber(data[2])

if tokens == nil then
    tokens = capacity
    last_refill = now
end

local elapsed = math.max(0, now - last_refill)
tokens = math.min(capacity, tokens + elapsed * refill_rate)

local allowed = 0
local retry_after_ms = 0

if tokens >= cost then
    tokens = tokens - cost
    allowed = 1
else
    local missing = cost - tokens
    if refill_rate > 0 then
        retry_after_ms = math.ceil((missing / refill_rate) * 1000)
    else
        retry_after_ms = -1
    end
end

redis.call('HMSET', KEYS[1], 'tokens', tokens, 'last_refill', now)
redis.call('EXPIRE', KEYS[1], 60)

return {allowed, tostring(tokens), retry_after_ms}
"""


@dataclass
class RateLimitVerdict:
    allowed: bool
    tokens_remaining: float
    retry_after_ms: int


async def check_rps(
    redis: Redis,
    agent: str,
    rps_limit: int,
    cost: float = 1.0,
) -> RateLimitVerdict:
    """Atomically consume ``cost`` tokens from the bucket for ``agent``.

    Args:
        redis: async Redis handle
        agent: agent name (each agent has an independent bucket)
        rps_limit: bucket capacity *and* refill rate per second
        cost: tokens to consume (default 1)
    """

    if rps_limit <= 0:
        return RateLimitVerdict(allowed=True, tokens_remaining=math.inf, retry_after_ms=0)

    key = f"request_layer:bucket:{agent}"
    now = time.time()
    raw = await redis.eval(
        _TOKEN_BUCKET_LUA,
        1,
        key,
        rps_limit,
        rps_limit,
        f"{now:.6f}",
        cost,
    )
    allowed_raw, tokens_raw, retry_raw = raw
    return RateLimitVerdict(
        allowed=bool(int(allowed_raw)),
        tokens_remaining=float(tokens_raw),
        retry_after_ms=int(retry_raw),
    )


def _minute_bucket(now: float | None = None) -> int:
    return int((now if now is not None else time.time()) // 60)


async def check_cost(
    redis: Redis,
    agent: str,
    cost_cap_usd_per_min: float,
) -> bool:
    """Return ``True`` if ``agent`` is currently *under* its cost cap."""

    if cost_cap_usd_per_min <= 0:
        return True
    bucket = _minute_bucket()
    key = f"request_layer:cost:{agent}:{bucket}"
    raw = await redis.get(key)
    if raw is None:
        return True
    try:
        spent = float(raw)
    except (TypeError, ValueError):
        return True
    return spent < cost_cap_usd_per_min


async def record_cost(redis: Redis, agent: str, cost_usd: float) -> float:
    """Add ``cost_usd`` to the current minute bucket and return new total."""

    if cost_usd <= 0:
        return 0.0
    bucket = _minute_bucket()
    key = f"request_layer:cost:{agent}:{bucket}"
    pipe = redis.pipeline()
    pipe.incrbyfloat(key, cost_usd)
    pipe.expire(key, 70)
    new_total, _ = await pipe.execute()
    return float(new_total)


# Approximate token rates (USD per token). Conservative defaults; real
# values come from the AgentCard ``model`` field via this lookup.
_RATE_TABLE: dict[str, tuple[float, float]] = {
    "gpt-4o": (0.0000025, 0.00001),
    "gpt-4o-mini": (0.00000015, 0.0000006),
    "gpt-4-turbo": (0.00001, 0.00003),
    "gpt-3.5-turbo": (0.0000005, 0.0000015),
    "claude-3-5-sonnet": (0.000003, 0.000015),
    "claude-3-haiku": (0.00000025, 0.00000125),
}


def estimate_cost(
    *,
    model: str | None,
    body_in: bytes | str,
    body_out: bytes | str,
    headers: dict[str, str] | None = None,
) -> float:
    """Estimate the LLM cost of a request/response pair.

    Honours ``X-Input-Tokens`` and ``X-Output-Tokens`` response headers if
    present (this is a hint Request layer would like agents to emit; absence
    causes us to fall back to a 4-chars-per-token estimate).
    """

    if headers is None:
        headers = {}

    input_tokens = _coerce_int(headers.get("X-Input-Tokens"))
    output_tokens = _coerce_int(headers.get("X-Output-Tokens"))

    if input_tokens is None:
        input_tokens = max(1, len(_to_str(body_in)) // 4)
    if output_tokens is None:
        output_tokens = max(1, len(_to_str(body_out)) // 4)

    rate_key = (model or "").lower()
    in_rate, out_rate = _RATE_TABLE.get(rate_key, (0.000001, 0.000003))
    return input_tokens * in_rate + output_tokens * out_rate


def _to_str(value: bytes | str) -> str:
    if isinstance(value, bytes):
        try:
            return value.decode("utf-8")
        except UnicodeDecodeError:
            return ""
    return value


def _coerce_int(value: str | None) -> int | None:
    if value is None:
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None
