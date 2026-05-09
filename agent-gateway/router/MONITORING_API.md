# Monitoring API Documentation

## Overview

The Nasiko Router provides comprehensive monitoring and operational control endpoints for managing cache, rate limiting, and system health. This unified request management layer sits between the gateway and agent fleet, providing intelligent caching and adaptive rate limiting.

## Architecture

```
┌─────────────┐
│   Client    │
└──────┬──────┘
       │
       ▼
┌─────────────────────────────────────────┐
│      Request Manager                    │
│  ┌─────────────┐    ┌────────────────┐ │
│  │ Rate Limiter│───▶│ Router         │ │
│  │ (queue)     │    │ Orchestrator   │ │
│  └─────────────┘    └────────┬───────┘ │
│                              │         │
│                              ▼         │
│                     ┌────────────────┐ │
│                     │ Cache Service  │ │
│                     │ (Redis/LRU)    │ │
│                     └────────────────┘ │
└─────────────────────────────────────────┘
                      │
                      ▼
              ┌───────────────┐
              │  Agent Fleet  │
              └───────────────┘
```

### Key Features

1. **Intelligent Caching**: Cache agent responses with Redis backend (LRU fallback)
   - Cache check happens AFTER agent selection but BEFORE HTTP call
   - Query normalization (lowercase, whitespace) for consistent cache keys
   - Per-agent cache statistics and management
   - TTL-based expiry (default 5 minutes)

2. **Adaptive Rate Limiting**: Token bucket algorithm with request queuing
   - Per-agent rate limits with configurable burst capacity
   - Requests queue when bucket empty (up to queue_size)
   - Raises `RateLimitExceeded` when queue full
   - Real-time metrics: tokens available, queue depth, acceptance/rejection rates

3. **Operational Controls**: RESTful monitoring endpoints
   - Cache management (stats, flush)
   - Rate limit configuration (per-agent overrides)
   - Health checks and dashboards

## API Endpoints

### Health & Status

#### `GET /health`
System health check with component status.

**Response:**
```json
{
  "router": "healthy",
  "timestamp": 1234567890.123,
  "components": {
    "cache": {
      "status": "healthy",
      "backend": "redis",
      "connected": true
    },
    "rate_limiter": {
      "status": "healthy"
    }
  }
}
```

#### `GET /router/health`
Simple router health check.

**Response:**
```json
{
  "status": "ok"
}
```

#### `GET /monitor/dashboard`
Combined dashboard with health, cache, and rate limiter stats.

**Response:**
```json
{
  "health": { /* health check response */ },
  "cache": { /* cache stats */ },
  "rate_limiter": { /* rate limiter stats */ }
}
```

---

### Cache Management

#### `GET /monitor/cache/stats`
Get overall cache statistics.

**Response:**
```json
{
  "backend": "redis",
  "connected": true,
  "hits": 150,
  "misses": 50,
  "hit_rate_pct": 75.0,
  "ttl_seconds": 300,
  "max_size": 1000
}
```

#### `GET /monitor/cache/stats/{agent_name}`
Get cache statistics for a specific agent.

**Path Parameters:**
- `agent_name` (string): Name of the agent

**Response:**
```json
{
  "agent": "code-agent",
  "hits": 45,
  "misses": 15,
  "hit_rate_pct": 75.0
}
```

#### `DELETE /monitor/cache`
Flush all cache entries.

**Response:**
```json
{
  "status": "ok",
  "cleared_keys": 123,
  "message": "Cache cleared successfully"
}
```

#### `DELETE /monitor/cache/{agent_name}`
Flush cache entries for a specific agent.

**Path Parameters:**
- `agent_name` (string): Name of the agent

**Response:**
```json
{
  "agent": "code-agent",
  "status": "ok",
  "cleared_keys": 45,
  "message": "Agent cache cleared successfully"
}
```

---

### Rate Limiting

#### `GET /monitor/rate-limits`
Get rate limit statistics for all agents.

**Response:**
```json
{
  "agents": {
    "code-agent": {
      "capacity": 10,
      "rate_per_second": 2.0,
      "tokens_available": 7.5,
      "queue_depth": 2,
      "queue_capacity": 20,
      "total_requests": 150,
      "accepted_requests": 145,
      "rejected_requests": 5,
      "rejection_rate_pct": 3.33,
      "avg_queue_wait_ms": 125.5
    }
  },
  "global_defaults": {
    "requests_per_second": 2.0,
    "burst_capacity": 10,
    "queue_size": 20,
    "queue_timeout": 30.0
  }
}
```

#### `GET /monitor/rate-limits/{agent_name}`
Get rate limit statistics for a specific agent.

**Path Parameters:**
- `agent_name` (string): Name of the agent

**Response:**
```json
{
  "agent": "code-agent",
  "capacity": 10,
  "rate_per_second": 2.0,
  "tokens_available": 7.5,
  "queue_depth": 2,
  "queue_capacity": 20,
  "total_requests": 150,
  "accepted_requests": 145,
  "rejected_requests": 5,
  "rejection_rate_pct": 3.33,
  "avg_queue_wait_ms": 125.5
}
```

**Error Response (404):**
```json
{
  "detail": "No rate limit stats found for agent: unknown-agent"
}
```

#### `GET /monitor/rate-limits/configs/list`
List all custom rate limit configurations.

**Response:**
```json
{
  "code-agent": {
    "requests_per_second": 5.0,
    "burst_capacity": 15,
    "queue_size": 30
  },
  "data-agent": {
    "requests_per_second": 1.0,
    "burst_capacity": 5,
    "queue_size": 10
  }
}
```

#### `PUT /monitor/rate-limits/{agent_name}`
Configure rate limits for a specific agent.

**Path Parameters:**
- `agent_name` (string): Name of the agent

**Request Body:**
```json
{
  "requests_per_second": 5.0,
  "burst_capacity": 15,
  "queue_size": 30
}
```

**Validation Rules:**
- `requests_per_second` must be > 0
- `burst_capacity` must be > 0
- `queue_size` must be >= 0

**Response:**
```json
{
  "agent": "code-agent",
  "status": "configured",
  "requests_per_second": 5.0,
  "burst_capacity": 15,
  "queue_size": 30
}
```

**Error Response (400):**
```json
{
  "detail": "requests_per_second must be greater than 0"
}
```

#### `DELETE /monitor/rate-limits/{agent_name}/config`
Remove custom rate limit configuration for an agent (revert to defaults).

**Path Parameters:**
- `agent_name` (string): Name of the agent

**Response (Success):**
```json
{
  "agent": "code-agent",
  "status": "removed",
  "message": "Rate limit configuration removed, using defaults"
}
```

**Response (Not Found):**
```json
{
  "agent": "code-agent",
  "status": "not_found",
  "message": "No custom configuration found for this agent"
}
```

---

### Legacy Metrics

#### `GET /metrics`
Legacy metrics endpoint (for backward compatibility).

**Response:**
```json
{
  "cache_hit_rate_pct": 75.0,
  "cache_hits": 150,
  "cache_misses": 50,
  "rate_limit_rejections": 5,
  "total_rate_limited_requests": 150
}
```

---

## Configuration

### Environment Variables

```bash
# Cache Configuration
CACHE_REDIS_URL=redis://localhost:6381/0
CACHE_TTL_SECONDS=300
CACHE_MAX_SIZE=1000

# Rate Limiting Configuration
RATE_LIMIT_REQUESTS_PER_SECOND=2.0
RATE_LIMIT_BURST_CAPACITY=10
RATE_LIMIT_QUEUE_SIZE=20
RATE_LIMIT_QUEUE_TIMEOUT=30.0
```

### Docker Compose

Redis cache service is included in `agent-gateway/docker-compose.yml`:

```yaml
nasiko-cache-redis:
  image: redis:7-alpine
  container_name: nasiko-cache-redis
  ports:
    - "6381:6379"
  command: redis-server --maxmemory 256mb --maxmemory-policy allkeys-lru
  networks:
    - nasiko-network
```

---

## Cache Behavior

### Cache Key Generation

Cache keys are generated using:
```
nasiko:cache:{sha256(agent_name|normalized_query)}
```

Where `normalized_query` is:
- Converted to lowercase
- Whitespace normalized (multiple spaces → single space)
- Trimmed

**Example:**
```python
# These queries produce the same cache key:
"Hello   World"
"hello world"
"HELLO WORLD"
```

### Cache Interception Point

Cache check happens at **Step 7** in the request pipeline:

1. Fetch agent cards from registry
2. Prepare agent data for routing
3. Create vector store for similarity search
4. Get conversation history
5. Route selection using AI (LLM selects agent)
6. Get agent URL
7. **→ Cache check** (if no files attached)
8. Forward to agent (if cache miss) and cache response

This ensures:
- Cache key uses the **exact agent name** selected by the LLM
- Gateway checks cache **before forwarding** to agents
- Files bypass cache (non-deterministic content)
- Error responses are not cached

### Cache Exclusions

The following are **not cached**:
- Requests with file attachments (non-deterministic)
- Error responses from agents
- Internal router messages

---

## Rate Limiting Behavior

### Token Bucket Algorithm

Each agent has a token bucket with:
- **Capacity**: Maximum burst size (tokens)
- **Refill Rate**: Tokens added per second
- **Queue**: Requests wait here when bucket is empty

**Flow:**
1. Request arrives
2. Try to consume 1 token from bucket
3. If token available → proceed immediately
4. If bucket empty → add to queue (up to `queue_size`)
5. If queue full → reject with `RateLimitExceeded` (503)

### Per-Agent Configuration

Rate limits can be configured per-agent:

```bash
# Configure code-agent with higher limits
curl -X PUT http://localhost:8000/monitor/rate-limits/code-agent \
  -H "Content-Type: application/json" \
  -d '{
    "requests_per_second": 5.0,
    "burst_capacity": 15,
    "queue_size": 30
  }'
```

### Default Limits

If no custom configuration exists, agents use global defaults:
- `requests_per_second`: 2.0
- `burst_capacity`: 10
- `queue_size`: 20
- `queue_timeout`: 30.0 seconds

---

## Success Metrics

### 1. Faster Repeated Responses
**Metric**: Reduction in response latency for repeated requests

**Measurement**:
```bash
# Check cache hit rate
curl http://localhost:8000/monitor/cache/stats

# Expected: hit_rate_pct > 50% for typical workloads
```

### 2. Reduced Duplicate Processing
**Metric**: Cache hit rate for repeated queries

**Measurement**:
```bash
# Per-agent cache stats
curl http://localhost:8000/monitor/cache/stats/code-agent

# Expected: hits > misses for frequently asked questions
```

### 3. Stable Overload Handling
**Metric**: Lower request failures during peak load with predictable queue times

**Measurement**:
```bash
# Check rate limit stats
curl http://localhost:8000/monitor/rate-limits

# Expected: rejection_rate_pct < 5%, avg_queue_wait_ms < 1000
```

### 4. Operational Visibility
**Metric**: Real-time monitoring dashboards

**Measurement**:
```bash
# Unified dashboard
curl http://localhost:8000/monitor/dashboard

# Expected: All components "healthy", metrics available
```

---

## Testing

Run the test suite:

```bash
cd agent-gateway/router
python -m pytest tests/test_request_management.py -v
```

**Test Coverage:**
- ✅ 59 tests passing
- ✅ LRU cache (7 tests)
- ✅ Cache key generation (4 tests)
- ✅ Cache service (10 tests)
- ✅ Rate limiter (9 tests)
- ✅ Request manager (11 tests)
- ✅ Monitoring endpoints (18 tests)

---

## Troubleshooting

### Cache Not Working

1. Check Redis connection:
```bash
curl http://localhost:8000/monitor/cache/stats
# Look for "connected": true
```

2. Verify Redis is running:
```bash
docker ps | grep nasiko-cache-redis
```

3. Check cache TTL hasn't expired:
```bash
# Default TTL is 300 seconds (5 minutes)
```

### Rate Limiting Too Aggressive

1. Check current limits:
```bash
curl http://localhost:8000/monitor/rate-limits/your-agent
```

2. Increase limits:
```bash
curl -X PUT http://localhost:8000/monitor/rate-limits/your-agent \
  -H "Content-Type: application/json" \
  -d '{
    "requests_per_second": 10.0,
    "burst_capacity": 20,
    "queue_size": 50
  }'
```

3. Monitor rejection rate:
```bash
# Should be < 5% for healthy operation
curl http://localhost:8000/monitor/rate-limits/your-agent | jq '.rejection_rate_pct'
```

### High Queue Wait Times

1. Check queue depth:
```bash
curl http://localhost:8000/monitor/rate-limits | jq '.agents[].queue_depth'
```

2. Increase burst capacity or rate:
```bash
curl -X PUT http://localhost:8000/monitor/rate-limits/your-agent \
  -H "Content-Type: application/json" \
  -d '{
    "requests_per_second": 5.0,
    "burst_capacity": 20,
    "queue_size": 30
  }'
```

---

## Implementation Files

- **Cache Service**: `src/core/cache_service.py`
- **Rate Limiter**: `src/core/rate_limiter.py`
- **Request Manager**: `src/services/request_manager.py`
- **Router Orchestrator**: `src/services/router_orchestrator.py`
- **Main API**: `src/main.py`
- **Configuration**: `src/config/settings.py`
- **Tests**: `tests/test_request_management.py`

---

## License

Part of the Nasiko multi-agent routing system.
