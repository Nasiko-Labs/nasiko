# Resilient Agent Request Layer - File Reference Guide

## Quick Navigation

### 📍 Location of All Files

All files are in the `/agent-gateway/` directory with the following structure:

```
agent-gateway/
├── RESILIENT_REQUEST_LAYER_DESIGN.md          ← Architecture & Design
├── RESILIENT_INTEGRATION_GUIDE.py             ← How to integrate
├── resilient_config.py                        ← Configuration profiles
├── DEPLOYMENT_AND_OPERATIONS.md               ← Ops & Deployment
├── router/
│   └── src/
│       └── resilient/                         ← CORE IMPLEMENTATION
│           ├── __init__.py                    ← Module exports
│           ├── cache_manager.py               ← Caching system
│           ├── rate_limiter.py                ← Rate limiting
│           ├── request_queue_manager.py       ← Queue management
│           ├── metrics_collector.py           ← Metrics collection
│           ├── request_layer.py               ← Main orchestrator
│           ├── routes.py                      ← REST API endpoints
│           ├── README.md                      ← User guide
│           └── tests.py                       ← Test suite
```

---

## 📄 File Descriptions

### Core Implementation Files

#### `router/src/resilient/cache_manager.py` (240 lines)
**Purpose**: Response caching with deduplication

**Key Classes:**
- `CacheManager` - Main caching interface

**Key Methods:**
- `get(agent_id, request_data)` - Retrieve cached response
- `set(agent_id, request_data, response, ttl_seconds)` - Cache response
- `flush_agent(agent_id)` - Clear agent's cache
- `stats(agent_id)` - Get cache statistics

**When to Use**: For understanding how caching works, or to modify caching behavior

---

#### `router/src/resilient/rate_limiter.py` (280 lines)
**Purpose**: Token bucket rate limiting per agent

**Key Classes:**
- `RateLimiter` - Token bucket implementation

**Key Methods:**
- `can_process(agent_id)` - Check rate limit (non-consuming)
- `acquire(agent_id)` - Consume token if available
- `set_default_limit(agent_id, rps, burst)` - Configure agent
- `reset_agent(agent_id)` - Reset to full capacity

**When to Use**: For understanding rate limiting, or to customize token refill logic

---

#### `router/src/resilient/request_queue_manager.py` (310 lines)
**Purpose**: Priority-based request queuing

**Key Classes:**
- `RequestQueueManager` - Queue implementation

**Key Methods:**
- `enqueue(agent_id, request_data, priority)` - Queue request
- `dequeue(agent_id, count)` - Get next requests
- `get_queue_status(agent_id)` - Queue statistics
- `clear_queue(agent_id)` - Empty queue

**When to Use**: For understanding queuing, or to modify queue behavior

---

#### `router/src/resilient/metrics_collector.py` (280 lines)
**Purpose**: Real-time metrics and statistics collection

**Key Classes:**
- `MetricsCollector` - Metrics collection interface

**Key Methods:**
- `record_hit(agent_id, response_time_ms)` - Record cache hit
- `record_miss(agent_id, response_time_ms)` - Record cache miss
- `get_stats(agent_id)` - Get agent statistics
- `get_summary()` - Get overall summary

**When to Use**: For understanding metrics, or to add custom metrics

---

#### `router/src/resilient/request_layer.py` (200 lines)
**Purpose**: Main orchestrator coordinating all components

**Key Classes:**
- `ResilientRequestLayer` - Main interface

**Key Methods:**
- `process_request(agent_id, request_data, agent_func)` - Process request
- `on_response_received(...)` - Handle response
- `configure_agent(agent_id, ...)` - Configure agent
- `health()` - Check system health

**When to Use**: For integrating with router, or for high-level orchestration

---

#### `router/src/resilient/routes.py` (280 lines)
**Purpose**: REST API endpoints for monitoring and control

**Key Classes:**
- `ResilientRequestLayerAPI` - API implementation
- `create_resilient_routes(request_layer)` - Factory function

**Available Endpoints** (25+ total):
- Cache: `/resilient/cache/stats`, `/resilient/cache/clear`
- Rate Limit: `/resilient/rate-limit/config`, `/resilient/rate-limit/update`
- Queue: `/resilient/queue/status`, `/resilient/queue/process`
- Metrics: `/resilient/metrics/stats`, `/resilient/metrics/reset`
- Health: `/resilient/health`
- Dashboard: `/resilient/dashboard`

**When to Use**: For operational monitoring and control

---

#### `router/src/resilient/__init__.py` (20 lines)
**Purpose**: Module exports and public API

**Exports:**
- `CacheManager`
- `RateLimiter`
- `RequestQueueManager`
- `MetricsCollector`
- `ResilientRequestLayer`

**When to Use**: For importing resilient layer components

---

#### `router/src/resilient/tests.py` (500+ lines)
**Purpose**: Comprehensive test suite

**Test Classes:**
- `TestCacheManager` - Cache tests
- `TestRateLimiter` - Rate limiting tests
- `TestRequestQueueManager` - Queue tests
- `TestMetricsCollector` - Metrics tests
- `TestResilientRequestLayer` - Integration tests
- `TestEdgeCases` - Error handling tests
- `TestPerformance` - Performance tests
- `TestIntegrationScenarios` - Real-world scenarios

**Running Tests:**
```bash
pytest router/src/resilient/tests.py -v
pytest router/src/resilient/tests.py::TestCacheManager -v
pytest router/src/resilient/tests.py -k "cache" -v
```

---

### Documentation Files

#### `RESILIENT_REQUEST_LAYER_DESIGN.md` (400+ lines)
**Purpose**: Architecture and design documentation

**Sections:**
- Overview and problem statement
- Architecture diagram
- Data model specification (Redis keys)
- Component descriptions
- API endpoint documentation
- Configuration options
- Error handling
- Future enhancements

**Read This For:** Understanding the complete system design

---

#### `RESILIENT_INTEGRATION_GUIDE.py` (300+ lines)
**Purpose**: Step-by-step integration instructions

**Sections:**
1. Updating main.py
2. Updating router_orchestrator.py
3. Integration with FastAPI
4. Configuration via environment variables
5. Agent-specific configuration
6. Monitoring setup

**Read This For:** How to integrate into existing code

---

#### `resilient_config.py` (400+ lines)
**Purpose**: Configuration profiles and examples

**Profiles:**
- Development environment (loose limits)
- Production environment (appropriate limits)
- Peak hours (increased capacity)
- Maintenance mode (reduced capacity)

**Functions:**
- `get_profile(agent_id)` - Get agent configuration
- `validate_config(config)` - Validate configuration
- `print_config_summary(config)` - Print summary

**Read This For:** How to configure agents

**Usage:**
```python
python resilient_config.py  # Prints configuration summary
```

---

#### `router/src/resilient/README.md` (600+ lines)
**Purpose**: Comprehensive user guide

**Sections:**
- Quick start
- Architecture overview
- Component documentation
- REST API reference
- Configuration options
- Integration guide
- Monitoring setup
- Best practices
- Troubleshooting
- Performance characteristics
- Future enhancements

**Read This For:** Complete reference guide

---

#### `DEPLOYMENT_AND_OPERATIONS.md` (500+ lines)
**Purpose**: Deployment and operational procedures

**Sections:**
1. Pre-deployment checklist
2. Deployment steps (staging → production)
3. Configuration (env vars, YAML, K8s)
4. Monitoring & Alerting (Prometheus, Grafana)
5. Daily/Weekly/Monthly operational tasks
6. Troubleshooting guide
7. Performance tuning
8. Disaster recovery

**Read This For:** How to deploy and operate in production

---

#### `/RESILIENT_SOLUTION_SUMMARY.md` (Root level - 400+ lines)
**Purpose**: Complete solution overview

**Sections:**
- What was built
- Architecture diagram
- Key features
- Performance characteristics
- Integration steps
- Expected outcomes
- File listing
- Next steps

**Read This For:** Executive summary and getting started guide

---

## 🎯 How to Use These Files

### For Architecture Understanding
1. Read `/RESILIENT_SOLUTION_SUMMARY.md` (overview)
2. Read `RESILIENT_REQUEST_LAYER_DESIGN.md` (detailed architecture)
3. Review diagrams in design document
4. Review component documentation in `README.md`

### For Integration
1. Review `RESILIENT_INTEGRATION_GUIDE.py` for code examples
2. Copy integration code to `main.py` and `router_orchestrator.py`
3. Configure agents using `resilient_config.py`
4. Add routes to FastAPI app
5. Test with provided test suite

### For Operational Setup
1. Review `DEPLOYMENT_AND_OPERATIONS.md` pre-deployment checklist
2. Follow deployment steps for staging
3. Set up monitoring per "Monitoring & Alerting" section
4. Configure alerting for key metrics
5. Follow roll-out procedure for production

### For Troubleshooting
1. Check `README.md` troubleshooting section
2. Monitor endpoints: `/resilient/health`, `/resilient/metrics/stats`
3. Review `DEPLOYMENT_AND_OPERATIONS.md` troubleshooting procedures
4. Enable debug logging in components
5. Check Redis directly for state inspection

### For Performance Tuning
1. Review "Performance Tuning" section in `DEPLOYMENT_AND_OPERATIONS.md`
2. Monitor cache hit ratio (`/resilient/cache/stats`)
3. Review rate limit configuration (`/resilient/rate-limit/config`)
4. Analyze queue depth (`/resilient/queue/status`)
5. Adjust configuration dynamically via API

### For Testing
1. Review test suite in `tests.py`
2. Run existing tests: `pytest router/src/resilient/tests.py`
3. Add project-specific tests
4. Run load tests before production
5. Monitor metrics during testing

---

## 📊 File Statistics

| Category | File | Lines | Purpose |
|----------|------|-------|---------|
| Core | cache_manager.py | 240 | Caching |
| Core | rate_limiter.py | 280 | Rate limiting |
| Core | request_queue_manager.py | 310 | Queuing |
| Core | metrics_collector.py | 280 | Metrics |
| Core | request_layer.py | 200 | Orchestration |
| Core | routes.py | 280 | REST API |
| Support | __init__.py | 20 | Exports |
| **Core Total** | **~1,610** | | |
| | | | |
| Testing | tests.py | 500+ | Test suite |
| | | | |
| Documentation | RESILIENT_REQUEST_LAYER_DESIGN.md | 400+ | Architecture |
| Documentation | RESILIENT_INTEGRATION_GUIDE.py | 300+ | Integration |
| Documentation | resilient_config.py | 400+ | Configuration |
| Documentation | README.md | 600+ | User guide |
| Documentation | DEPLOYMENT_AND_OPERATIONS.md | 500+ | Ops manual |
| Documentation | RESILIENT_SOLUTION_SUMMARY.md | 400+ | Summary |
| **Documentation Total** | **~2,600** | | |
| **Grand Total** | **~4,700 lines** | | |

---

## 🔗 Key Integration Points

### In `main.py`
```python
# Add imports
from router.src.resilient import ResilientRequestLayer
from router.src.resilient.routes import create_resilient_routes

# Initialize at startup
resilient_layer = ResilientRequestLayer(...)

# Add routes
app.include_router(create_resilient_routes(resilient_layer))
```

### In `router_orchestrator.py`
```python
# Pass to orchestrator
async for response in orch._handle_route_selection(...):
    # Check cache, rate limits, queue
    response, was_cached, status = self.resilient_layer.process_request(...)
    
    # Cache response after agent call
    self.resilient_layer.on_response_received(...)
```

### Environment Variables
```env
REDIS_RESILIENT_DB=1
CACHE_DEFAULT_TTL_SECONDS=3600
RATE_LIMIT_DEFAULT_RPS=10.0
```

---

## 🚀 Quick Start Command

```bash
# Read these in order:
1. cat /RESILIENT_SOLUTION_SUMMARY.md
2. cat agent-gateway/RESILIENT_REQUEST_LAYER_DESIGN.md
3. cat agent-gateway/RESILIENT_INTEGRATION_GUIDE.py
4. cat agent-gateway/router/src/resilient/README.md
5. cat agent-gateway/DEPLOYMENT_AND_OPERATIONS.md

# Then integrate:
# - Copy code examples from RESILIENT_INTEGRATION_GUIDE.py
# - Configure with resilient_config.py
# - Deploy per DEPLOYMENT_AND_OPERATIONS.md

# Validate:
pytest agent-gateway/router/src/resilient/tests.py -v
curl http://localhost:8000/resilient/health
curl http://localhost:8000/resilient/dashboard
```

---

## 📝 Document Cross-References

| Topic | Primary File | Supporting Files |
|-------|--------------|------------------|
| Architecture | RESILIENT_REQUEST_LAYER_DESIGN.md | /RESILIENT_SOLUTION_SUMMARY.md |
| Integration | RESILIENT_INTEGRATION_GUIDE.py | routes.py, request_layer.py |
| Configuration | resilient_config.py | DEPLOYMENT_AND_OPERATIONS.md |
| Operations | DEPLOYMENT_AND_OPERATIONS.md | README.md |
| API Reference | routes.py, README.md | RESILIENT_REQUEST_LAYER_DESIGN.md |
| Troubleshooting | README.md | DEPLOYMENT_AND_OPERATIONS.md |
| Testing | tests.py, README.md | RESILIENT_INTEGRATION_GUIDE.py |
| Performance | README.md | DEPLOYMENT_AND_OPERATIONS.md |

---

## ✅ Validation Checklist

### Before Using Files
- [ ] All files created in correct locations
- [ ] Python syntax valid (can parse all .py files)
- [ ] Markdown syntax valid (can open all .md files)
- [ ] Cross-references are correct

### Before Integration
- [ ] Understanding of architecture (read design doc)
- [ ] Redis instance available
- [ ] Python dependencies installed (redis-py)
- [ ] Environment variables configured

### Before Deployment
- [ ] All tests passing
- [ ] Integration code reviewed
- [ ] Configuration finalized
- [ ] Monitoring setup complete
- [ ] Alerting configured

### Before Production
- [ ] Staging deployment successful
- [ ] Load testing completed
- [ ] Monitoring verified
- [ ] Runbooks created
- [ ] Team trained

---

## 🆘 Help & Support

| Question | Answer In |
|----------|-----------|
| How does caching work? | cache_manager.py + README.md |
| How does rate limiting work? | rate_limiter.py + README.md |
| How does queuing work? | request_queue_manager.py + README.md |
| How do I integrate? | RESILIENT_INTEGRATION_GUIDE.py |
| How do I configure? | resilient_config.py |
| How do I monitor? | DEPLOYMENT_AND_OPERATIONS.md |
| How do I troubleshoot? | README.md + DEPLOYMENT_AND_OPERATIONS.md |
| What tests exist? | tests.py |
| What APIs are available? | routes.py + README.md |
| What's the architecture? | RESILIENT_REQUEST_LAYER_DESIGN.md |

---

Generated: $(date)  
For: nasiko AI Agent Platform  
Purpose: Resilient Agent Request Layer Implementation
