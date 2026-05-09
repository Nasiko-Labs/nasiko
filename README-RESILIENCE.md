# Nasiko Resilience Layer - Buildathon Submission

## Architecture
Native Nasiko microservice providing intelligent traffic control between Kong Gateway and agent fleet.

## Features
- **Semantic Cache**: Vector similarity matching reduces LLM calls by up to 69%
- **Adaptive Rate Limiting**: Priority-aware token buckets with cross-tier borrowing
- **Predictive Circuit Breaker**: Opens at 50% errors, before total failure
- **Real-time Dashboard**: Live metrics with cost savings calculator
- **Graceful Degradation**: Pass-through mode when Redis unavailable
- **Phoenix Integration**: Full distributed tracing observability

## Quick Start
```bash
# Already running via docker compose
curl http://localhost:8090/resilience/health
curl http://localhost:8090/resilience/metrics
# Dashboard: http://localhost:8090/resilience/metrics/dashboard
```
