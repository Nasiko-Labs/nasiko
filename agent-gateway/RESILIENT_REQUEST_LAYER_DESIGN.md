# Resilient Agent Request Layer Design

## Overview
The resilient request layer is a middleware system that sits between the gateway router and agent fleet, providing:
1. **Response Caching** - Cache agent responses to avoid redundant compute
2. **Per-Agent Rate Limiting** - Token bucket algorithm with adaptive limits
3. **Request Queuing** - Queue excess traffic instead of immediate rejection
4. **Operational Monitoring** - REST endpoints for cache/limit management and stats

## Architecture

### Layer Position in Request Flow
```
Client Request
    ↓
Router Service (existing)
    ↓
RequestLayer Middleware [NEW]
    ├→ Check cache (hit? return cached response)
    ├→ Check rate limits (within quota? proceed : queue)
    ├→ Forward to Agent
    └→ Cache response + update metrics
    ↓
Agent Response → Client
```

### Data Model in Redis

#### Cache Keys
```
agent:{agent_id}:cache:{request_hash} 
  → {
      "response": <agent_response>,
      "ttl": <seconds>,
      "created_at": <timestamp>,
      "hit_count": <int>
    }
```

#### Rate Limit Keys
```
rate_limit:{agent_id}
  → {
      "tokens": <current_tokens>,
      "capacity": <max_tokens>,
      "refill_rate": <tokens_per_second>,
      "last_refill": <timestamp>
    }

rate_limit_config:{agent_id}
  → {
      "requests_per_second": <float>,
      "burst_capacity": <int>,
      "queue_enabled": <bool>,
      "max_queue_size": <int>
    }
```

#### Request Queue
```
queue:{agent_id}
  → [
      {
        "request_id": <uuid>,
        "request_data": <json>,
        "queued_at": <timestamp>,
        "priority": <int (0-10, higher=more urgent)>
      },
      ...
    ]
```

#### Metrics
```
metrics:{agent_id}
  → {
      "total_requests": <int>,
      "cache_hits": <int>,
      "cache_misses": <int>,
      "requests_queued": <int>,
      "requests_rejected": <int>,
      "avg_response_time_ms": <float>,
      "last_updated": <timestamp>,
      "current_queue_size": <int>
    }
```

## Components

### 1. CacheManager
- **Purpose**: Handle response caching with TTL
- **Methods**:
  - `get(agent_id, request_hash)` → cached_response | None
  - `set(agent_id, request_hash, response, ttl_seconds)` → bool
  - `delete(agent_id, request_hash)` → bool
  - `flush_agent(agent_id)` → deleted_count
  - `flush_all()` → deleted_count
  - `stats(agent_id)` → cache_stats

### 2. RateLimiter
- **Purpose**: Token bucket algorithm with per-agent configuration
- **Methods**:
  - `can_process(agent_id)` → bool
  - `acquire(agent_id, tokens=1)` → bool
  - `refill(agent_id)` → current_tokens
  - `update_limit(agent_id, rps, burst)` → bool
  - `get_current_state(agent_id)` → state_dict
  - `reset_agent(agent_id)` → bool

### 3. RequestQueueManager
- **Purpose**: Queue excess requests with priority support
- **Methods**:
  - `enqueue(agent_id, request, priority=5)` → queue_position | None
  - `dequeue(agent_id, count=1)` → list[queued_requests]
  - `get_queue_status(agent_id)` → queue_state
  - `clear_queue(agent_id)` → cleared_count
  - `peek(agent_id)` → next_request | None

### 4. MetricsCollector
- **Purpose**: Track request statistics and performance metrics
- **Methods**:
  - `record_hit(agent_id, response_time_ms)` → None
  - `record_miss(agent_id, response_time_ms)` → None
  - `record_queued(agent_id)` → None
  - `record_rejected(agent_id)` → None
  - `record_error(agent_id, error_type)` → None
  - `get_stats(agent_id)` → stats_dict
  - `get_all_stats()` → {agent_id: stats_dict}

### 5. RequestLayerMiddleware
- **Purpose**: Orchestrate the full request flow
- **Methods**:
  - `process_request(agent_id, request) → (response, was_cached)`
  - `process_queued_requests(agent_id)` → processed_count

## API Endpoints (Monitoring & Operations)

### GET /resilient/cache/stats
Returns cache statistics for all agents or filtered by agent_id
```json
{
  "agents": {
    "agent_id_1": {
      "total_cached_responses": 150,
      "cache_hit_ratio": 0.65,
      "avg_cache_ttl_seconds": 3600,
      "memory_usage_bytes": 1024000
    }
  },
  "total_memory_bytes": 5000000
}
```

### POST /resilient/cache/clear
Clear cache for specific agent or all agents
```json
{
  "agent_id": "agent_1" // optional, clears all if omitted
}
```

### GET /resilient/rate-limit/config
Get current rate limit configuration
```json
{
  "agents": {
    "agent_id_1": {
      "requests_per_second": 10.0,
      "burst_capacity": 50,
      "queue_enabled": true,
      "max_queue_size": 100
    }
  }
}
```

### POST /resilient/rate-limit/update
Update rate limit for an agent
```json
{
  "agent_id": "agent_1",
  "requests_per_second": 20.0,
  "burst_capacity": 100,
  "queue_enabled": true,
  "max_queue_size": 200
}
```

### POST /resilient/rate-limit/reset
Reset rate limiter for an agent
```json
{
  "agent_id": "agent_1"
}
```

### GET /resilient/queue/status
Get queue status for agents
```json
{
  "agents": {
    "agent_id_1": {
      "queue_size": 5,
      "oldest_request_age_ms": 2500,
      "estimated_wait_time_ms": 5000
    }
  }
}
```

### POST /resilient/queue/process
Manually trigger queue processing for an agent
```json
{
  "agent_id": "agent_1",
  "max_requests": 10
}
```

### GET /resilient/metrics/stats
Get comprehensive metrics for all agents
```json
{
  "agents": {
    "agent_id_1": {
      "total_requests": 1000,
      "cache_hits": 650,
      "cache_misses": 350,
      "cache_hit_ratio": 0.65,
      "requests_queued": 15,
      "requests_rejected": 2,
      "avg_response_time_ms": 245.5,
      "current_queue_size": 3
    }
  },
  "timestamp": "2025-05-09T10:30:00Z"
}
```

## Configuration

### Environment Variables
```env
# Cache Configuration
CACHE_DEFAULT_TTL_SECONDS=3600
CACHE_MAX_SIZE_MB=1000
CACHE_EVICTION_POLICY=lru  # lru, lfu, fifo

# Rate Limiting
RATE_LIMIT_DEFAULT_RPS=10.0
RATE_LIMIT_DEFAULT_BURST=50
RATE_LIMIT_QUEUE_ENABLED=true
RATE_LIMIT_MAX_QUEUE_SIZE=100

# Request Processing
REQUEST_BATCH_SIZE=10
REQUEST_PROCESS_INTERVAL_MS=100

# Redis
REDIS_REQUEST_LAYER_DB=1  # separate from main app
```

## Implementation Details

### Request Hash Calculation
Hash is computed from:
- agent_id
- request method (GET, POST, etc.)
- request path
- request body (sorted JSON)
- relevant headers (exclude timestamps, request IDs)

This ensures identical logical requests hit the cache regardless of minor variations.

### Token Bucket Algorithm
```
available_tokens = min(
  capacity,
  previous_tokens + (time_since_last_refill * refill_rate)
)

if available_tokens >= 1:
  consume 1 token
  allow request
else:
  if queue_enabled and queue.size < max_queue_size:
    enqueue request
  else:
    reject request
```

### Metrics Update Frequency
- Cache hit/miss: immediate
- Response time: sampled every 100 requests
- Queue size: event-driven on enqueue/dequeue
- Overall stats: aggregated on demand

## Error Handling

### Cache Failures
- Redis connection loss: bypass cache, process normally
- Serialization errors: log and continue without caching
- TTL expiration: automatic via Redis TTL

### Rate Limit Failures
- Redis connection loss: allow all requests temporarily, log warning
- Corrupted state: reset to clean state
- Lost tokens: conservative approach, require explicit recovery

### Queue Failures
- Queue persistence failure: log error, drop request with retry recommendation
- Duplicate processing: use request_id deduplication
- Lost queue items: TTL on queue items, max retention 1 hour

## Deployment Notes

1. **Redis Instance**: Use separate DB (default DB 1) from main app
2. **Backward Compatibility**: Disabled by default via env var
3. **Monitoring**: Expose Prometheus metrics at `/metrics/resilient`
4. **Health Check**: Include cache availability in `/health` endpoint

## Future Enhancements

1. **Distributed Cache** - Multi-node Redis cluster support
2. **Predictive Scaling** - ML-based load prediction for proactive rate limit adjustment
3. **Circuit Breaking** - Auto-adjust limits if downstream agents fail
4. **Cache Warming** - Pre-populate cache based on usage patterns
5. **Cost Tracking** - Per-agent cost metrics based on compute/LLM usage
