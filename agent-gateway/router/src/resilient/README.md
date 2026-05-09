# Resilient Agent Request Layer

A production-ready request management system for AI agent platforms that combines intelligent caching, per-agent rate limiting, and request queuing to prevent redundant compute and cascading overload.

## Overview

Modern AI platforms orchestrate dozens of specialized agents concurrently. Without proper traffic controls, two critical failure modes emerge:

1. **Redundant Compute** - Identical queries being repeatedly processed by agents
2. **Cascading Overload** - Traffic spikes to one agent affecting overall platform stability

This system provides unified request management that sits between the gateway and agent fleet, combining:

- **Response Caching** - Cache agent responses to avoid recomputing identical requests
- **Per-Agent Rate Limiting** - Token bucket algorithm with per-agent configuration
- **Request Queuing** - Queue excess traffic instead of rejecting it
- **Operational Monitoring** - REST endpoints for cache/limit management and real-time stats

## Quick Start

### Installation

The system is implemented in `/router/src/resilient/` and requires:
- Redis (for distributed state)
- Python 3.8+
- FastAPI
- redis-py

```bash
pip install redis fastapi uvicorn
```

### Basic Usage

```python
from router.src.resilient import ResilientRequestLayer

# Initialize the resilient layer
resilient = ResilientRequestLayer(
    redis_db=1,
    cache_ttl_seconds=3600,
    default_rps=10.0,  # 10 requests per second
    default_burst=50,   # Allow 50 token burst
)

# Configure per-agent limits
resilient.configure_agent(
    "compliance_checker",
    requests_per_second=5.0,
    burst_capacity=25,
    cache_ttl_seconds=7200,
    max_queue_size=50
)

# Check cache and rate limits before forwarding to agent
response, was_cached, status = resilient.process_request(
    agent_id="compliance_checker",
    request_data={"query": "Check this code"},
    agent_func=None,
)

if was_cached:
    # Serve from cache
    return response

if status.startswith("queued"):
    # Request was queued
    return {"status": "queued", "position": status}

if status.startswith("rejected"):
    # Request was rejected
    return {"error": "Agent at capacity"}, 429

# Forward to agent
agent_response = await call_agent(...)

# Cache the response
resilient.on_response_received(
    agent_id="compliance_checker",
    request_data={"query": "Check this code"},
    response=agent_response,
    response_time_ms=245.5
)
```

## Architecture

### Component Diagram

```
┌─────────────────────────────────────────────────┐
│         Gateway / Router Service                 │
└─────────────────────────────────────────────────┘
                      ↓
┌──────────────────────────────────────────────────────────┐
│       Resilient Request Layer Middleware                  │
├─────────────────────┬─────────────────────┬──────────────┤
│  CacheManager       │  RateLimiter        │  Queue       │
│  - Hit/Miss cache   │  - Token bucket     │  - Priority  │
│  - TTL aware        │  - Per-agent config │  - Size limit│
│  - Metrics          │  - Burst capacity   │  - Aging     │
└─────────────────────┴─────────────────────┴──────────────┘
          ↓                   ↓                    ↓
    ┌──────────────────────────────────────────────────┐
    │         Redis (Shared State Backend)              │
    │  - Cache entries with TTL                         │
    │  - Rate limit token buckets                       │
    │  - Request queues (sorted by priority)            │
    │  - Metrics and statistics                         │
    └──────────────────────────────────────────────────┘
```

### Request Flow

```
User Request
    ↓
[1] Check Cache
    ├→ Hit? Return cached response (fast path)
    └→ Miss? Continue
    ↓
[2] Check Rate Limit
    ├→ Within quota? Acquire token(s)
    └→ Exceeded? Continue to queue
    ↓
[3] Queue Decision
    ├→ Queue enabled and space available? Enqueue request
    └→ Queue full? Reject request (429 Too Many Requests)
    ↓
[4] Forward to Agent
    ├→ Get response from agent
    └→ Record response time
    ↓
[5] Cache Response
    ├→ Store in cache with TTL
    └→ Update metrics (hit, response time)
    ↓
Return Response to User
```

## Core Components

### CacheManager

Handles response caching with intelligent deduplication.

**Key Methods:**
- `get(agent_id, request_data)` - Retrieve cached response
- `set(agent_id, request_data, response, ttl_seconds)` - Cache response
- `flush_agent(agent_id)` - Clear cache for agent
- `flush_all()` - Clear all cache
- `stats(agent_id)` - Get cache statistics

**Features:**
- Request hash calculation (ignores volatile fields like timestamps)
- Configurable TTL per response
- Automatic expiration via Redis TTL
- Cache hit/miss metrics

### RateLimiter

Token bucket algorithm with per-agent configuration.

**Key Methods:**
- `can_process(agent_id)` - Check if quota available (doesn't consume)
- `acquire(agent_id, tokens=1)` - Consume tokens if available
- `set_default_limit(agent_id, rps, burst_capacity)` - Configure limits
- `reset_agent(agent_id)` - Reset to full capacity

**Features:**
- Automatic token refill based on elapsed time
- Burst capacity support for traffic spikes
- Per-agent independent limits
- Thread-safe token management

**Token Bucket Algorithm:**
```
available_tokens = min(
    capacity,
    previous_tokens + (time_since_last_refill * refill_rate)
)

if available_tokens >= 1:
    consume token
    allow request
else:
    rate_limited
```

### RequestQueueManager

Priority-based request queuing for load balancing.

**Key Methods:**
- `enqueue(agent_id, request, priority)` - Queue a request
- `dequeue(agent_id, count)` - Get next requests
- `peek(agent_id)` - View next request without removing
- `get_queue_status(agent_id)` - Queue statistics
- `clear_queue(agent_id)` - Empty queue

**Features:**
- Priority support (0-10 scale)
- Size limits to prevent unbounded growth
- Request aging tracking
- Wait time estimation

### MetricsCollector

Real-time metrics and statistics collection.

**Key Methods:**
- `record_hit(agent_id, response_time_ms)` - Record cache hit
- `record_miss(agent_id, response_time_ms)` - Record cache miss
- `record_queued(agent_id)` - Record queued request
- `record_rejected(agent_id)` - Record rejected request
- `get_stats(agent_id)` - Get agent statistics
- `get_all_stats()` - Get all agent statistics
- `get_summary()` - Get system-wide summary

**Metrics Tracked:**
- Cache hits/misses and hit ratio
- Average response time (moving average)
- Queued and rejected requests
- Queue size and wait times
- Per-agent and aggregate statistics

### ResilientRequestLayer

Main orchestrator coordinating all components.

**Key Methods:**
- `process_request()` - Main request processing
- `on_response_received()` - Cache and metrics on response
- `process_queue()` - Manually trigger queue processing
- `configure_agent()` - Set up agent-specific limits
- `reset_agent()` - Clear all state for agent
- `health()` - Health check of all components
- `get_comprehensive_stats()` - Full system stats

## REST API Endpoints

### Health & Status

```
GET /resilient/health
```
Check health of all components.

```json
{
  "cache": {"status": "healthy"},
  "rate_limiter": {"status": "healthy"},
  "queue": {"status": "healthy"},
  "metrics": {"status": "healthy"}
}
```

### Cache Operations

```
GET /resilient/cache/stats?agent_id=xxx
```
Get cache statistics. Omit `agent_id` for all agents.

```json
{
  "agent": "compliance_checker",
  "stats": {
    "cache_hits": 150,
    "cache_misses": 100,
    "total_requests": 250,
    "cache_hit_ratio": 0.6
  }
}
```

```
POST /resilient/cache/clear
Body: {"agent_id": "compliance_checker"}
```
Clear cache for an agent.

```
POST /resilient/cache/ttl?agent_id=compliance_checker
Body: {"query": "Translate Hello"}
```
Get remaining cache TTL in seconds for that exact request.

### Rate Limiting

```
GET /resilient/rate-limit/config
```
Get rate limit configuration for all agents.

```
GET /resilient/rate-limit/state?agent_id=xxx
```
Get current token state for an agent.

```
POST /resilient/rate-limit/update
Body: {
  "agent_id": "compliance_checker",
  "requests_per_second": 5.0,
  "burst_capacity": 25
}
```

```
POST /resilient/rate-limit/reset
Body: {"agent_id": "compliance_checker"}
```
Reset rate limiter to full capacity.

### Queue Management

```
GET /resilient/queue/status
```
Get queue status for all agents.

```
GET /resilient/queue/status/{agent_id}
```
Get queue status for specific agent.

```json
{
  "queue_size": 5,
  "max_queue_size": 100,
  "queue_utilization": 0.05,
  "oldest_request_age_ms": 2500,
  "estimated_wait_time_ms": 5000,
  "is_full": false
}
```

```
POST /resilient/queue/process
Body: {"agent_id": "compliance_checker", "max_requests": 10}
```
Manually trigger queue processing.

```
POST /resilient/queue/clear
Body: {"agent_id": "compliance_checker"}
```
Clear queue for an agent.

### Metrics & Monitoring

```
GET /resilient/metrics/stats
```
Get comprehensive metrics for all agents.

```
GET /resilient/metrics/stats/{agent_id}
```
Get metrics for specific agent.

```json
{
  "total_requests": 1000,
  "cache_hits": 650,
  "cache_misses": 350,
  "cache_hit_ratio": 0.65,
  "requests_queued": 15,
  "requests_rejected": 2,
  "avg_response_time_ms": 245.5
}
```

```
POST /resilient/metrics/reset?agent_id=xxx
```
Reset metrics. Omit `agent_id` to reset all agents.

### Agent Management

```
POST /resilient/agent/configure
Body: {
  "agent_id": "compliance_checker",
  "requests_per_second": 5.0,
  "burst_capacity": 25,
  "cache_ttl_seconds": 7200,
  "max_queue_size": 50
}
```
Configure all aspects of an agent.

```
POST /resilient/agent/reset
Body: {"agent_id": "compliance_checker"}
```
Reset all state for an agent.

### Dashboard

```
GET /resilient/dashboard
```
Get comprehensive dashboard data with all metrics, configs, and queue status.

## Configuration

### Environment Variables

```env
# Redis Configuration
REDIS_RESILIENT_DB=1
REDIS_HOST=localhost
REDIS_PORT=6379

# Cache Configuration
CACHE_DEFAULT_TTL_SECONDS=3600
CACHE_EVICTION_POLICY=lru  # lru, lfu, fifo

# Rate Limiting
RATE_LIMIT_DEFAULT_RPS=10.0
RATE_LIMIT_DEFAULT_BURST=50
RATE_LIMIT_QUEUE_ENABLED=true

# Request Queuing
RATE_LIMIT_MAX_QUEUE_SIZE=100
REQUEST_BATCH_SIZE=10
REQUEST_PROCESS_INTERVAL_MS=100
```

### Per-Agent Configuration

```python
AGENT_LIMITS = {
    "compliance_checker": {
        "rps": 5.0,
        "burst": 25,
        "cache_ttl": 7200,
        "max_queue": 50
    },
    "github_agent": {
        "rps": 20.0,
        "burst": 100,
        "cache_ttl": 3600,
        "max_queue": 200
    },
}

# Apply at startup
for agent_id, config in AGENT_LIMITS.items():
    resilient_layer.configure_agent(
        agent_id,
        requests_per_second=config["rps"],
        burst_capacity=config["burst"],
        cache_ttl_seconds=config["cache_ttl"],
        max_queue_size=config["max_queue"],
    )
```

## Integration with Router Service

See `RESILIENT_INTEGRATION_GUIDE.py` for detailed integration steps.

Key integration points:

1. **Initialize resilient layer in main.py**
2. **Pass to RouterOrchestrator**
3. **Check cache before forwarding requests**
4. **Cache responses after agent processing**
5. **Include resilient routes in FastAPI app**

## Monitoring & Observability

### Metrics to Monitor

**System Health:**
- Redis connection status
- Component health checks
- Memory usage

**Performance:**
- Cache hit ratio (target: > 50%)
- Average response time
- p95/p99 latencies

**Capacity:**
- Queue size and fill percentage
- Rate limit token consumption
- Requests queued vs. processed

**Business:**
- Total requests per agent
- Time saved through caching
- Agents hitting rate limits

### Example Monitoring Queries

```bash
# Overall hit ratio
curl http://localhost:8000/resilient/metrics/stats | jq '.overall_cache_hit_ratio'

# Agent queue depth
curl http://localhost:8000/resilient/queue/status | jq '.agents[] | select(.queue_size > 0)'

# Current token state
curl http://localhost:8000/resilient/rate-limit/state?agent_id=compliance_checker

# System health
curl http://localhost:8000/resilient/health
```

## Best Practices

### Configuration

1. **Start Conservative** - Begin with lower RPS limits and gradually increase based on metrics
2. **Cache TTL Strategy** - Use shorter TTL (30-60 min) for dynamic data, longer (2-4 hours) for stable data
3. **Queue Sizing** - Set max queue size to 2-3x burst capacity
4. **Burst Capacity** - Allow 5-10x instantaneous spike over sustained RPS

### Operational

1. **Monitor Hit Ratio** - Target > 50% hit ratio for cache optimization
2. **Watch Queue Depth** - Sustained queue > 20% capacity indicates need for higher limits
3. **Review Response Times** - Track p50/p95/p99 to identify degradation
4. **Regular Resets** - Periodically reset metrics to catch issues in specific time windows

### Integration

1. **Graceful Degradation** - If Redis unavailable, bypass resilient layer (fall through to agent)
2. **Circuit Breaking** - Monitor agent error rates and auto-scale limits downward if failing
3. **Cost Tracking** - Use metrics to calculate cost savings from caching (compute avoided)

## Advanced Topics

### Request Hash Calculation

The caching system uses SHA256 hashing of sanitized request data to ensure identical logical requests match, while ignoring:

- Timestamps (`timestamp`, `created_at`)
- Request IDs (`request_id`, `trace_id`, `span_id`)
- Volatile headers

This ensures the cache works for repeated queries while avoiding false hits.

### Token Bucket Algorithm Details

The token bucket implementation:

1. Tracks `tokens`, `capacity`, `refill_rate`, `last_refill_time`
2. On each check, calculates elapsed time since last refill
3. Adds `elapsed_seconds * refill_rate` tokens (capped at capacity)
4. Consumes 1 token per request if available
5. All operations are O(1) with Redis access

### Queue Priority Sorting

Queued requests are stored in a Redis sorted set with:

- Score: `-priority` (negated so higher priority sorts first)
- Value: JSON-serialized request object
- Sorted set provides O(log N) insertion/deletion

### Metrics Aggregation

Metrics use exponential moving average:

```
new_avg = (current_avg * (n-1) + new_value) / n
```

This gives recent values more weight while still considering full history.

## Troubleshooting

### Cache Not Working

**Check:**
1. Redis connectivity: `GET /resilient/health`
2. Cache statistics: `GET /resilient/cache/stats`
3. Redis TTL: Verify keys have expiration set
4. Request hash consistency: Debug hash calculation

### Rate Limiting Not Applied

**Check:**
1. Agent configuration: `GET /resilient/rate-limit/config`
2. Token state: `GET /resilient/rate-limit/state?agent_id=xxx`
3. Rate limit is being checked in request flow
4. Verify `can_process()` or `acquire()` is called

### Queue Growing Unbounded

**Check:**
1. Queue configuration: `GET /resilient/queue/status`
2. Max queue size limit is set
3. Queue processing is being triggered
4. Agents are actually processing requests from queue

### High Memory Usage

**Check:**
1. Cache size: `GET /resilient/cache/stats`
2. Number of cached responses
3. TTL settings - may be too long
4. Redis memory info: `INFO memory`

## Performance Characteristics

| Operation | Time Complexity | Space Complexity | Notes |
|-----------|-----------------|------------------|-------|
| Cache lookup | O(1) | O(requests) | Redis GET |
| Rate limit check | O(1) | O(agents) | Single hash lookup |
| Enqueue request | O(log N) | O(queue_size) | Redis ZADD |
| Dequeue request | O(log N) | O(queue_size) | Redis ZRANGE/ZREM |
| Get metrics | O(1) | O(agents) | Hash aggregation |
| Health check | O(1) | O(1) | Redis PING |

**Typical Latencies (Redis local):**
- Cache check: < 1ms
- Rate limit check: < 1ms
- Queue operation: 1-2ms
- Metrics update: < 1ms

## Future Enhancements

1. **Distributed Caching** - Multi-node Redis cluster support
2. **Predictive Scaling** - ML-based load prediction for proactive rate limit adjustment
3. **Circuit Breaking** - Auto-adjust limits if downstream agents fail
4. **Cache Warming** - Pre-populate cache based on historical patterns
5. **Cost Tracking** - Per-agent cost metrics based on compute/LLM usage
6. **GraphQL Support** - Intelligent caching for GraphQL queries
7. **Compression** - Compress large cached responses
8. **Security** - Per-tenant isolation, API key rate limiting

## License

See main repository license.

## Support

For issues or questions:
1. Check monitoring endpoints for component health
2. Review metrics to understand system state
3. Enable debug logging to trace request flow
4. See integration guide for architectural questions
