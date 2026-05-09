# Quick Reference: Monitoring API

## 🚀 Most Common Operations

### Check System Health
```bash
curl http://localhost:8000/monitor/dashboard
```

### View Cache Performance
```bash
# Overall stats
curl http://localhost:8000/monitor/cache/stats

# Per-agent stats
curl http://localhost:8000/monitor/cache/stats/code-agent
```

### View Rate Limiting
```bash
# All agents
curl http://localhost:8000/monitor/rate-limits

# Single agent
curl http://localhost:8000/monitor/rate-limits/code-agent
```

### Configure Rate Limits
```bash
# Increase limits for high-traffic agent
curl -X PUT http://localhost:8000/monitor/rate-limits/code-agent \
  -H "Content-Type: application/json" \
  -d '{
    "requests_per_second": 10.0,
    "burst_capacity": 20,
    "queue_size": 50
  }'
```

### Clear Cache
```bash
# Clear all cache
curl -X DELETE http://localhost:8000/monitor/cache

# Clear specific agent cache
curl -X DELETE http://localhost:8000/monitor/cache/code-agent
```

---

## 📊 Key Metrics to Monitor

### Cache Hit Rate
```bash
curl http://localhost:8000/monitor/cache/stats | jq '.hit_rate_pct'
# Target: > 50%
```

### Rate Limit Rejection Rate
```bash
curl http://localhost:8000/monitor/rate-limits | jq '.agents[].rejection_rate_pct'
# Target: < 5%
```

### Average Queue Wait Time
```bash
curl http://localhost:8000/monitor/rate-limits | jq '.agents[].avg_queue_wait_ms'
# Target: < 1000ms
```

---

## 🔧 Configuration

### Environment Variables
```bash
# Cache
CACHE_REDIS_URL=redis://localhost:6381/0
CACHE_TTL_SECONDS=300
CACHE_MAX_SIZE=1000

# Rate Limiting
RATE_LIMIT_REQUESTS_PER_SECOND=2.0
RATE_LIMIT_BURST_CAPACITY=10
RATE_LIMIT_QUEUE_SIZE=20
RATE_LIMIT_QUEUE_TIMEOUT=30.0
```

### Start Redis
```bash
cd agent-gateway
docker-compose up -d nasiko-cache-redis
```

---

## 🧪 Testing

### Run All Tests
```bash
cd agent-gateway/router
python -m pytest tests/test_request_management.py -v
```

### Expected Result
```
59 passed in ~60s
```

---

## 🚨 Troubleshooting

### Cache Not Working
```bash
# Check Redis connection
curl http://localhost:8000/monitor/cache/stats | jq '.connected'
# Should be: true

# Check Redis is running
docker ps | grep nasiko-cache-redis
```

### Too Many Rate Limit Rejections
```bash
# Check current limits
curl http://localhost:8000/monitor/rate-limits/your-agent

# Increase limits
curl -X PUT http://localhost:8000/monitor/rate-limits/your-agent \
  -H "Content-Type: application/json" \
  -d '{
    "requests_per_second": 10.0,
    "burst_capacity": 20,
    "queue_size": 50
  }'
```

### High Queue Wait Times
```bash
# Check queue depth
curl http://localhost:8000/monitor/rate-limits | jq '.agents[].queue_depth'

# Increase burst capacity
curl -X PUT http://localhost:8000/monitor/rate-limits/your-agent \
  -H "Content-Type: application/json" \
  -d '{
    "requests_per_second": 5.0,
    "burst_capacity": 20,
    "queue_size": 30
  }'
```

---

## 📚 Full Documentation

- **API Reference**: `MONITORING_API.md`
- **Solution Overview**: `BUILDTHON_SOLUTION.md`
- **Implementation Details**: `IMPLEMENTATION_SUMMARY.md`

---

## 🎯 Success Criteria

| Metric | Target | Endpoint |
|--------|--------|----------|
| Cache Hit Rate | > 50% | `/monitor/cache/stats` |
| Rejection Rate | < 5% | `/monitor/rate-limits` |
| Queue Wait Time | < 1000ms | `/monitor/rate-limits` |
| System Health | "healthy" | `/monitor/dashboard` |

---

## 💡 Pro Tips

1. **Monitor cache hit rate daily** - Low hit rate means queries aren't repeating
2. **Set alerts on rejection rate** - High rejections mean limits too tight
3. **Configure per-agent limits** - Different agents have different capacities
4. **Clear cache after agent updates** - Prevents serving stale responses
5. **Use dashboard endpoint** - Single call for all metrics

---

## 🔗 Quick Links

- Health: http://localhost:8000/health
- Dashboard: http://localhost:8000/monitor/dashboard
- Cache Stats: http://localhost:8000/monitor/cache/stats
- Rate Limits: http://localhost:8000/monitor/rate-limits
- Metrics: http://localhost:8000/metrics
