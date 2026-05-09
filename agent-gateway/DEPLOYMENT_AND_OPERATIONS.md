# Resilient Request Layer - Deployment & Operations Guide

## Table of Contents

1. [Pre-Deployment Checklist](#pre-deployment-checklist)
2. [Deployment Steps](#deployment-steps)
3. [Configuration](#configuration)
4. [Monitoring & Alerting](#monitoring--alerting)
5. [Operational Tasks](#operational-tasks)
6. [Troubleshooting](#troubleshooting)
7. [Performance Tuning](#performance-tuning)
8. [Disaster Recovery](#disaster-recovery)

---

## Pre-Deployment Checklist

Before deploying the resilient request layer to production:

### Infrastructure
- [ ] Redis instance running and accessible (separate from main app, DB 1+)
- [ ] Redis persistent storage configured (RDB or AOF)
- [ ] Redis memory limits set appropriately (recommend 2-4 GB minimum)
- [ ] Redis replication/sentinel configured for HA
- [ ] Network connectivity verified between router and Redis

### Code & Configuration
- [ ] Resilient layer code reviewed and tested
- [ ] Default agent profiles defined in `resilient_config.py`
- [ ] Environment variables configured for target environment
- [ ] Integration points in `router_orchestrator.py` implemented
- [ ] Monitoring endpoints registered and accessible
- [ ] API documentation updated

### Testing
- [ ] Unit tests passing (cache, rate limiter, queue, metrics)
- [ ] Integration tests with mock agents passing
- [ ] Load testing completed (see load testing scenarios)
- [ ] Cache hit ratio verified with realistic traffic patterns
- [ ] Rate limiting tested with traffic spikes
- [ ] Queue behavior tested at capacity limits

### Operational Readiness
- [ ] Alerting configured (Slack, PagerDuty, etc.)
- [ ] Monitoring dashboards created (Grafana, DataDog, etc.)
- [ ] Runbooks created for common incidents
- [ ] Team trained on monitoring endpoints
- [ ] Backup and recovery procedures documented

---

## Deployment Steps

### Step 1: Deploy to Staging

```bash
# 1. Build/push Docker image
docker build -t router-resilient:v1 agent-gateway/
docker push registry.example.com/router-resilient:v1

# 2. Deploy to staging cluster
kubectl apply -f agent-gateway/k8s/staging/resilient-deployment.yaml

# 3. Run smoke tests
pytest agent-gateway/router/src/resilient/tests.py -v

# 4. Monitor logs for errors
stern router resilient -f | grep -i error
```

### Step 2: Validate Staging

```bash
# Check health
curl http://staging-router:8000/resilient/health

# Verify Redis connectivity
curl http://staging-router:8000/resilient/cache/stats

# Run load test
python -m pytest tests/load_test_resilient.py --workers=20 --duration=300
```

### Step 3: Gradually Roll Out to Production

```bash
# Use canary deployment - start with 10% of traffic
kubectl patch deployment router -p \
  '{"spec":{"template":{"metadata":{"annotations":{"version":"resilient-v1"}}}}}'

# Monitor metrics for 30 minutes
METRICS_ENDPOINT="http://prod-router:8000/resilient/metrics/stats"
while true; do
  curl -s $METRICS_ENDPOINT | jq '.overall_cache_hit_ratio'
  sleep 60
done

# If healthy, increase to 50%
# If issues, roll back to 0%
```

### Step 4: Full Production Rollout

```bash
# Increase to 100% once metrics look good for 2+ hours
kubectl rollout status deployment/router -w

# Verify all agents configured
curl http://prod-router:8000/resilient/rate-limit/config | jq '.agents | keys'
```

---

## Configuration

### Environment Variables for Docker/K8s

```env
# Redis
REDIS_RESILIENT_HOST=redis-cluster.prod
REDIS_RESILIENT_PORT=6379
REDIS_RESILIENT_DB=1
REDIS_RESILIENT_PASSWORD=xxx

# Cache
CACHE_DEFAULT_TTL_SECONDS=3600
CACHE_EVICTION_POLICY=lru

# Rate Limiting
RATE_LIMIT_DEFAULT_RPS=10.0
RATE_LIMIT_DEFAULT_BURST=50

# Startup Configuration
LOAD_AGENT_CONFIG_FROM_FILE=true
AGENT_CONFIG_FILE=/config/agent_profiles.yaml
```

### Agent Configuration YAML

```yaml
# /config/agent_profiles.yaml
agents:
  compliance_checker:
    requests_per_second: 5.0
    burst_capacity: 25
    cache_ttl_seconds: 7200
    max_queue_size: 50
    enabled: true
  
  github_agent:
    requests_per_second: 15.0
    burst_capacity: 75
    cache_ttl_seconds: 1800
    max_queue_size: 150
    enabled: true
  
  translator:
    requests_per_second: 20.0
    burst_capacity: 100
    cache_ttl_seconds: 3600
    max_queue_size: 200
    enabled: true

defaults:
  requests_per_second: 10.0
  burst_capacity: 50
  cache_ttl_seconds: 3600
  max_queue_size: 100
```

### K8s Deployment Example

```yaml
# agent-gateway/k8s/production/resilient-deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: router-resilient
  namespace: prod
spec:
  replicas: 3
  selector:
    matchLabels:
      app: router
  template:
    metadata:
      labels:
        app: router
        version: resilient-v1
    spec:
      containers:
      - name: router
        image: registry.example.com/router-resilient:v1
        ports:
        - containerPort: 8000
          name: http
        - containerPort: 8001
          name: metrics
        env:
        - name: REDIS_RESILIENT_HOST
          valueFrom:
            configMapKeyRef:
              name: resilient-config
              key: redis-host
        - name: REDIS_RESILIENT_DB
          value: "1"
        resources:
          requests:
            memory: "512Mi"
            cpu: "500m"
          limits:
            memory: "1Gi"
            cpu: "1000m"
        livenessProbe:
          httpGet:
            path: /health
            port: 8000
          initialDelaySeconds: 30
          periodSeconds: 10
        readinessProbe:
          httpGet:
            path: /resilient/health
            port: 8000
          initialDelaySeconds: 10
          periodSeconds: 5
        volumeMounts:
        - name: config
          mountPath: /config
      volumes:
      - name: config
        configMap:
          name: resilient-agent-profiles
```

---

## Monitoring & Alerting

### Key Metrics to Monitor

| Metric | Target | Alert If |
|--------|--------|----------|
| Cache Hit Ratio | > 50% | < 30% |
| Avg Response Time | < 500ms | > 2000ms |
| Queue Size | < 20% capacity | > 80% capacity |
| Error Rate | < 1% | > 5% |
| Redis Memory | < 80% | > 85% |
| Rate Limit Violations | Few | Consistently high |

### Prometheus Metrics

```yaml
# Add to prometheus.yml
scrape_configs:
  - job_name: 'router-resilient'
    static_configs:
      - targets: ['localhost:8000']
    metrics_path: '/metrics/resilient'
    scrape_interval: 15s
```

### Example Prometheus Queries

```promql
# Cache hit ratio (last 5 minutes)
rate(resilient_cache_hits_total[5m]) / 
rate(resilient_cache_requests_total[5m])

# Queue backlog
resilient_queue_size{agent_id="compliance_checker"}

# Rate limit violations per agent per minute
rate(resilient_rate_limit_violations_total[1m])

# P95 response time
histogram_quantile(0.95, rate(resilient_response_time_seconds_bucket[5m]))
```

### Grafana Dashboard JSON

```json
{
  "dashboard": {
    "title": "Resilient Request Layer",
    "panels": [
      {
        "title": "Cache Hit Ratio (%)",
        "targets": [
          {
            "expr": "rate(resilient_cache_hits_total[5m]) / rate(resilient_cache_requests_total[5m]) * 100"
          }
        ]
      },
      {
        "title": "Queue Depth by Agent",
        "targets": [
          {
            "expr": "resilient_queue_size"
          }
        ]
      },
      {
        "title": "Response Time P95 (ms)",
        "targets": [
          {
            "expr": "histogram_quantile(0.95, rate(resilient_response_time_seconds_bucket[5m])) * 1000"
          }
        ]
      },
      {
        "title": "Rate Limit Violations/min",
        "targets": [
          {
            "expr": "rate(resilient_rate_limit_violations_total[1m])"
          }
        ]
      }
    ]
  }
}
```

### Alert Rules

```yaml
# alerts/resilient-rules.yaml
groups:
- name: resilient_request_layer
  rules:
  - alert: LowCacheHitRatio
    expr: resilient_cache_hit_ratio < 0.3
    for: 5m
    annotations:
      summary: "Cache hit ratio low"
  
  - alert: QueueBacklog
    expr: resilient_queue_size{agent_id="compliance_checker"} > 40
    for: 5m
    annotations:
      summary: "Queue backlog for {{ $labels.agent_id }}"
  
  - alert: HighErrorRate
    expr: rate(resilient_errors_total[5m]) > 0.05
    for: 5m
    annotations:
      summary: "Error rate above 5%"
  
  - alert: RedisMemoryHigh
    expr: resilient_redis_memory_used_bytes / resilient_redis_memory_max_bytes > 0.85
    for: 10m
    annotations:
      summary: "Redis memory usage above 85%"
```

---

## Operational Tasks

### Daily Operations

```bash
# 1. Check system health (before/after peak hours)
curl http://prod-router:8000/resilient/health

# 2. Review cache performance
curl http://prod-router:8000/resilient/metrics/stats | \
  jq '.overall_cache_hit_ratio'

# 3. Check for queue buildup
curl http://prod-router:8000/resilient/queue/status | \
  jq '.agents[] | select(.queue_utilization > 0.5)'

# 4. Monitor rate limit activity
curl http://prod-router:8000/resilient/metrics/stats | \
  jq '.agents[].requests_rejected' | sort -rn | head -5
```

### Weekly Operations

```bash
# 1. Review metrics trends
# - Create weekly report of cache hit ratio by agent
# - Identify agents with low hit ratio (< 30%)
# - Check queue depth trends

# 2. Performance analysis
# - Review p50/p95/p99 response times
# - Identify slow agents and investigate
# - Check if agent limits need adjustment

# 3. Cost analysis
# - Calculate compute saved through caching
# - Analyze cache efficiency (hits × cost saved per hit)
# - Review most cached vs least cached agents

# 4. Configuration review
# - Are rate limits still appropriate?
# - Do any agents need higher RPS?
# - Are cache TTLs optimal?
```

### Monthly Maintenance

```bash
# 1. Reset metrics
curl -X POST http://prod-router:8000/resilient/metrics/reset

# 2. Redis maintenance
redis-cli --cluster info prod-redis-cluster
redis-cli --cluster check prod-redis-cluster

# 3. Review logs for errors
# Query ELK/Splunk for past 7 days:
# service:router AND module:resilient AND level:error

# 4. Capacity planning
# - Review peak hour metrics
# - Project growth for next quarter
# - Identify scale limits

# 5. Configuration optimization
# - Analyze which agents benefit most from caching
# - Adjust rate limits based on actual usage
# - Fine-tune burst capacity percentages
```

### Quarterly Review

```bash
# 1. Performance review
# - Cache hit ratio trends
# - Response time trends
# - Queue depth trends

# 2. Scaling assessment
# - Are we hitting limits?
# - Do we need more Redis capacity?
# - Should we adjust default profiles?

# 3. Cost-benefit analysis
# - Quantify savings from caching
# - Analyze impact of rate limiting on user experience
# - ROI of the resilient request layer

# 4. Roadmap planning
# - Plan upgrades for next period
# - Identify feature requests
# - Plan team training
```

---

## Troubleshooting

### Issue: Low Cache Hit Ratio (< 30%)

**Symptoms:** Cache hit ratio below 30% despite caching being enabled.

**Diagnosis:**
1. Check if requests are actually cacheable
2. Verify TTL is appropriately configured
3. Check request hash calculation

**Steps:**
```bash
# 1. Check cache configuration
curl http://prod-router:8000/resilient/cache/stats

# 2. Verify requests are being cached
# Enable debug logging and check logs

# 3. Review TTL settings for low-hit agents
curl http://prod-router:8000/resilient/rate-limit/config | \
  jq '.agents | to_entries[] | select(.value.cache_hit_ratio < 0.3)'

# 4. Increase TTL for agents with legitimate repeated queries
curl -X POST http://prod-router:8000/resilient/agent/configure \
  -d '{"agent_id": "xxx", "cache_ttl_seconds": 7200}'
```

### Issue: Queue Growing Unbounded

**Symptoms:** Queue size continuously increasing, approaching capacity.

**Diagnosis:**
1. Rate limits too strict for actual load
2. Not processing queue fast enough
3. Downstream agent failing

**Steps:**
```bash
# 1. Check queue status
curl http://prod-router:8000/resilient/queue/status

# 2. Check agent metrics
curl http://prod-router:8000/resilient/metrics/stats | \
  jq '.agents[] | select(.queue_size > 20)'

# 3. Check agent health (is it accepting requests?)
# Monitor agent directly

# 4. Increase rate limit (if we can)
curl -X POST http://prod-router:8000/resilient/rate-limit/update \
  -d '{"agent_id": "xxx", "requests_per_second": 20.0}'

# 5. Or process queue faster
curl -X POST http://prod-router:8000/resilient/queue/process \
  -d '{"agent_id": "xxx", "max_requests": 50}'
```

### Issue: High Memory Usage in Redis

**Symptoms:** Redis memory usage above 80%, performance degrading.

**Diagnosis:**
1. Cache TTL too long
2. Too many requests being cached
3. Cache entries too large

**Steps:**
```bash
# 1. Check cache size
curl http://prod-router:8000/resilient/cache/stats | \
  jq '.agents[] | .memory_usage_bytes' | \
  awk '{sum+=$1} END {print sum/1024/1024 " MB"}'

# 2. Find agents with large caches
redis-cli --stat size
redis-cli SCAN 0 TYPE string | \
  awk '{print $1}' | \
  xargs redis-cli DEBUG OBJECT | \
  sort -k2 -rn | head

# 3. Reduce TTL for high-memory agents
for agent in $(curl -s http://prod-router:8000/resilient/cache/stats | \
                jq -r '.agents | keys[]'); do
  curl -X POST http://prod-router:8000/resilient/agent/configure \
    -d "{\"agent_id\": \"$agent\", \"cache_ttl_seconds\": 1800}"
done

# 4. Clear cache if emergency
curl -X POST http://prod-router:8000/resilient/cache/clear
```

### Issue: Redis Connection Lost

**Symptoms:** Resilient layer endpoints returning errors, caching/rate limiting not working.

**Diagnosis:**
1. Redis unreachable
2. Connection pool exhausted
3. Network issues

**Steps:**
```bash
# 1. Check Redis connectivity
redis-cli -h prod-redis ping

# 2. Check firewall/network
netstat -an | grep 6379
telnet prod-redis 6379

# 3. Check Redis logs
tail -f /var/log/redis/redis-server.log

# 4. Restart Redis (if safe)
redis-cli shutdown
systemctl start redis-server

# 5. Monitor fallback behavior (should still work)
# System should degrade gracefully without Redis
```

---

## Performance Tuning

### Optimize Cache Hit Ratio

```python
# 1. Analyze which requests are repeated
# Use metrics endpoint to identify high-volume agents

# 2. Increase TTL for stable data
# Example: Compliance rules don't change for days
resilient.configure_agent(
    "compliance_checker",
    cache_ttl_seconds=86400  # 24 hours
)

# 3. Verify deduplication works
# Check that similar requests hash to same value

# 4. Monitor impact
# Track cache_hit_ratio before/after changes
```

### Optimize Rate Limiting

```python
# 1. Measure actual throughput
# Check queue depth during peak hours

# 2. Adjust RPS to match capacity
# If queue empty: increase RPS
# If queue growing: decrease RPS

# 3. Fine-tune burst capacity
# Should be 5-10x RPS for short spikes

resilient.configure_agent(
    "github_agent",
    requests_per_second=25.0,  # Increase from 15
    burst_capacity=150,        # Increase from 75
)
```

### Optimize Queue Processing

```python
# 1. Monitor queue age
# If oldest_request_age_ms > threshold, increase processing rate

# 2. Manual processing during bottlenecks
curl -X POST http://prod-router:8000/resilient/queue/process \
  -d '{"agent_id": "compliance_checker", "max_requests": 100}'

# 3. Increase burst capacity to allow faster queue drain
```

### Redis Optimization

```bash
# 1. Configure Redis memory policy
redis-cli CONFIG SET maxmemory-policy allkeys-lru

# 2. Enable persistence
redis-cli CONFIG SET save "900 1 300 1000 60 10000"

# 3. Enable compression (if large values)
redis-cli CONFIG SET appendfsync everysec

# 4. Monitor with INFO command
redis-cli INFO stats
redis-cli INFO memory
```

---

## Disaster Recovery

### Backup Strategy

```bash
# Daily backups of Redis (includes all cache, rate limit state)
# Store in S3/GCS with retention of 30 days

0 2 * * * /opt/scripts/backup-redis.sh

# Script content:
#!/bin/bash
BACKUP_DIR="/backups/redis"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
redis-cli BGSAVE
cp /var/lib/redis/dump.rdb $BACKUP_DIR/dump-$TIMESTAMP.rdb
gsutil cp $BACKUP_DIR/dump-$TIMESTAMP.rdb gs://backups/redis/
```

### Recovery Procedures

#### Complete Cache Loss

```bash
# 1. System continues working (cache misses increase)
# 2. Restore from backup
redis-cli SHUTDOWN NOSAVE
cp /backups/redis/dump-latest.rdb /var/lib/redis/dump.rdb
systemctl start redis-server

# 3. Monitor recovery
# Cache hit ratio will be 0 until rebuilt
curl http://prod-router:8000/resilient/metrics/stats | jq '..*'
```

#### Rate Limit State Loss

```bash
# 1. All agents reset to full burst capacity
# 2. May cause temporary overload spike
# 3. Monitor closely after recovery

# Reset limits appropriately
for agent in compliance_checker github_agent translator; do
  curl -X POST http://prod-router:8000/resilient/rate-limit/reset \
    -d "{\"agent_id\": \"$agent\"}"
done
```

#### Queue Loss

```bash
# 1. Queued requests are lost
# 2. If important, retry policy should catch them
# 3. Monitor for rejected requests

# Resetting queue
curl -X POST http://prod-router:8000/resilient/queue/clear \
  -d '{"agent_id": "all"}'

# Trigger reprocessing of lost requests
# (implementation-specific)
```

### Failover Procedures

```bash
# 1. Detect Redis failure
# Monitoring should alert

# 2. Start secondary Redis instance
systemctl start redis-server-secondary

# 3. Update router configuration
kubectl patch configmap resilient-config \
  -p '{"data":{"redis-host":"redis-secondary"}}'

# 4. Restart router pods
kubectl rollout restart deployment/router

# 5. Restore from backup once primary is back
timeline:
- T+0: Primary failure detected
- T+2: Secondary active
- T+5: Requests flowing to secondary
- T+30: Primary fixed
- T+35: Back to primary (after sync)
```

---

## Security Considerations

### Network Security

```yaml
# Restrict Redis access to router IPs only
kind: NetworkPolicy
metadata:
  name: redis-access
spec:
  podSelector:
    matchLabels:
      app: redis
  ingress:
  - from:
    - podSelector:
        matchLabels:
          app: router
    ports:
    - port: 6379
```

### Data Security

```bash
# 1. Encrypt Redis in transit
# Use TLS for Redis connections
REDIS_TLS_ENABLED=true

# 2. Set Redis password
redis-cli CONFIG SET requirepass $(openssl rand -base64 32)

# 3. Encrypt sensitive data before caching
# Use application-level encryption for sensitive responses

# 4. Audit cache access
# Log who accessed what (for compliance)
```

### Rate Limiting Security

```python
# 1. Prevent rate limit bypass
# Always check rate limit before processing

# 2. Per-client rate limiting (future enhancement)
# Current: per-agent only
# Future: per-api-key, per-user, per-ip

# 3. Distributed rate limiting
# Handle multi-server deployments correctly
```

---

## Conclusion

The resilient request layer dramatically improves platform reliability and efficiency. Follow these deployment and operational procedures to ensure smooth operation and maximum benefit.

For questions or issues, contact the DevOps/SRE team or refer to the main README in `/router/src/resilient/README.md`.
