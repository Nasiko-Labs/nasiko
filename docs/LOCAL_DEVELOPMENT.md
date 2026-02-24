# Local Development with Nasiko

This guide covers setting up and managing the Nasiko platform for local development using Docker Compose.

## Overview

The new `nasiko local` command group provides a simplified way to manage the local development stack:

- **Core services** (backend, gateway, web) use pre-built images from Docker registry
- **Agent services** are built on-the-fly from source code with automatic instrumentation
- **Single docker-compose file** consolidates all services for easy management
- **Unified network** (nasiko-network) for inter-service communication

## Quick Start

### 1. Setup Environment

```bash
# Copy environment template
cp .env.local.example .env

# Edit .env with your settings (optional for basic usage)
# Default values work for local development
```

### 2. Start the Stack

```bash
# Start all services
nasiko local up

# View logs
nasiko local logs -f

# Check status
nasiko local status
```

After startup, you'll see a table with available services:

```
Kong Gateway              http://localhost:9100      API gateway & agent routing
Backend API              http://localhost:8000/docs  Nasiko API (Swagger)
Konga UI                 http://localhost:1337       Kong management UI
Service Registry         http://localhost:8080       Agent discovery API
Router                   http://localhost:8081       Query routing service
Auth Service             http://localhost:8082       Authentication service
Chat History             http://localhost:8083       Chat history API
Web UI                   http://localhost:4000       Nasiko web interface
LangTrace                http://localhost:3000       Observability dashboard
```

### 3. Deploy Your First Agent

```bash
# Deploy an agent from local directory
nasiko local deploy-agent my-agent ./agents/my-agent

# Or use the CLI directly
nasiko agent upload-directory ./agents/my-agent --name my-agent
```

### 4. Test End-to-End Flow

```bash
# Open web UI
open http://localhost:4000

# Or make a direct API call to your agent
curl -X POST http://localhost:9100/my-agent/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "hello"}'
```

## Command Reference

### Start Services

```bash
# Start stack (default: build images first)
nasiko local up

# Start without building images
nasiko local up --no-build

# Run in foreground (see logs in real-time)
nasiko local up --attach
```

### Stop Services

```bash
# Stop stack (keep volumes)
nasiko local down

# Stop and remove all volumes (WARNING: data loss!)
nasiko local down --volumes
```

### View Logs

```bash
# View logs for all services
nasiko local logs

# View logs for specific service
nasiko local logs nasiko-backend

# Follow logs (-f) with last 50 lines (-n 50)
nasiko local logs -f -n 50 nasiko-router
```

### Service Management

```bash
# Check status
nasiko local status

# Restart all services
nasiko local restart

# Restart specific service
nasiko local restart nasiko-backend

# Open shell in service
nasiko local shell nasiko-backend
```

### Deploy Agents

```bash
# Deploy agent from directory
nasiko local deploy-agent my-agent ./agents/my-agent

# View agent registry
curl http://localhost:8080/agents

# View specific agent
curl http://localhost:8080/agents/my-agent
```

## Configuration

### Environment Variables

Create `.env` file (or `.nasiko.env`, `.nasiko-local.env`) with:

```bash
# Database credentials
MONGO_ROOT_USER=admin
MONGO_ROOT_PASSWORD=password

# API keys (optional)
OPENAI_API_KEY=sk-proj-...
GITHUB_CLIENT_ID=...
GITHUB_CLIENT_SECRET=...

# Registry configuration
NASIKO_REGISTRY_URL=nasiko              # Pre-built core services
NASIKO_VERSION=latest
AGENT_REGISTRY_URL=docker.io            # Agent images (local builds)

# Frontend configuration
REACT_APP_API_URL=http://localhost:9100
```

See `.env.local.example` for complete reference with descriptions.

### Port Configuration

Key ports used by default:

| Port | Service | Purpose |
|------|---------|---------|
| 9100 | Kong Gateway | Main API entry point |
| 8000 | Backend | Nasiko API server |
| 4000 | Web | Frontend UI |
| 8080 | Service Registry | Agent discovery |
| 3000 | LangTrace | Observability |
| 27017 | MongoDB | Database |
| 6379 | Redis | Cache/messaging |

To change ports, modify `docker-compose.nasiko.yml`.

## Troubleshooting

### Stack won't start

```bash
# Check port conflicts
sudo lsof -i :9100    # Check Kong port
sudo lsof -i :8000    # Check Backend port
sudo lsof -i :27017   # Check MongoDB port

# Check Docker daemon
docker ps

# View detailed logs
nasiko local logs nasiko-backend
nasiko local logs kong-gateway
```

### Can't connect to services

```bash
# Verify stack is running
nasiko local status

# Check network
docker network inspect nasiko-network

# Test connectivity between containers
docker exec nasiko-backend curl http://kong:8000/status
```

### Agent deployment fails

```bash
# View worker logs
nasiko local logs nasiko-worker

# Check agent structure
ls -la ./agents/my-agent/
# Must contain: Dockerfile, docker-compose.yml

# View deployment status
curl http://localhost:8000/api/v1/registries?name=my-agent
```

### Memory/CPU issues

Stack uses significant resources. If experiencing slowness:

```bash
# View Docker stats
docker stats

# Increase Docker resources:
# - Docker Desktop: Settings > Resources > increase Memory/CPU
# - Linux: Check system resources (docker run --memory 4g)

# Run minimal stack (skip observability)
# Edit docker-compose.nasiko.yml and comment out:
# - langtrace services
# - otel-collector
```

## Advanced Usage

### Custom Agent Development

1. **Create agent directory** (must match container name):
```bash
mkdir agents/my-custom-agent
cd agents/my-custom-agent
```

2. **Create Dockerfile** with your application

3. **Create docker-compose.yml**:
```yaml
services:
  my-custom-agent:
    build: .
    container_name: my-custom-agent
    ports:
      - "8090:5000"
    networks:
      - nasiko-network

networks:
  nasiko-network:
    external: true
    name: nasiko-network
```

4. **Deploy**:
```bash
nasiko local deploy-agent my-custom-agent ./agents/my-custom-agent
```

### Using Ollama Locally

To use local LLMs via Ollama:

```bash
# Add to docker-compose.nasiko.yml or run separately
docker run -d -p 11434:11434 --name ollama ollama/ollama

# Pull a model
docker exec ollama ollama pull llama2

# Set environment variable
export OLLAMA_SERVER=http://host.docker.internal:11434
```

### Accessing Databases

```bash
# MongoDB shell
docker exec -it mongodb mongosh mongodb://admin:password@localhost:27017

# Redis CLI
docker exec -it redis redis-cli

# PostgreSQL (Kong DB)
docker exec -it kong-database psql -U kong -d kong
```

### Observability

#### LangTrace Dashboard

Access at: `http://localhost:3000`

Configure in your agent:
```python
from langtrace_python_sdk import langtrace

langtrace.init(
    api_key="demo-api-key",
    api_host="http://host.docker.internal:3000/api"
)
```

#### OpenTelemetry Metrics

Collector listens on:
- gRPC: `localhost:4317`
- HTTP: `localhost:4318`

### Performance Optimization

**Image Caching**: Agent images are cached locally with naming convention `{agent_name}_instrumented`. Re-deploying the same agent reuses the cached image (fast path).

```bash
# View cached agent images
docker images | grep instrumented

# Clear cache (forces rebuild on next deploy)
docker rmi my-agent_instrumented
```

## Migration from Old Orchestrator

Old workflow (deprecated):
```bash
# Run orchestrator.py in one terminal
uv run orchestrator/orchestrator.py

# Run redis listener in another terminal
uv run orchestrator/redis_stream_listener.py

# Manually manage multiple compose files
# - app/docker-compose.app.yaml
# - agent-gateway/docker-compose.yml
# - observability/langtrace/docker-compose.langtrace.yaml
```

New workflow (recommended):
```bash
# Single command
nasiko local up

# Services auto-start with unified configuration
# Orchestrator runs inside stack as nasiko-worker service
```

## Network Architecture

```
┌─────────────────────────────────────────────────────────┐
│           nasiko-network (Docker bridge)                │
├─────────────────────────────────────────────────────────┤
│                                                         │
│  ┌──────────────────┐      ┌──────────────────┐       │
│  │  Kong Gateway    │      │   Kong DB        │       │
│  │  (port 9100)     ├─────▶│  (postgres)      │       │
│  └────────┬─────────┘      └──────────────────┘       │
│           │                                             │
│  ┌────────▼──────────────────────────────────────┐    │
│  │  Kong Service Registry                         │    │
│  │  (discovers agent containers via Docker)       │    │
│  └──────────────────────────────────────────────┘    │
│           │                                             │
│  ┌────────▼──────────────────────────────────────┐    │
│  │  Agent Services (deployed via orchestrator)    │    │
│  │  - my-agent (docker-compose.yml)              │    │
│  │  - another-agent (docker-compose.yml)         │    │
│  └──────────────────────────────────────────────┘    │
│                                                         │
│  ┌──────────────────┐      ┌──────────────────┐       │
│  │ Nasiko Backend   │      │   MongoDB        │       │
│  │ (port 8000)      ├─────▶│   Redis          │       │
│  └──────────────────┘      └──────────────────┘       │
│                                                         │
│  ┌──────────────────────────────────────────────┐    │
│  │  Supporting Services                          │    │
│  │  - Router, Auth Service, Chat History        │    │
│  │  - Web Frontend                               │    │
│  │  - Observability (LangTrace, OTEL)           │    │
│  └──────────────────────────────────────────────┘    │
│                                                         │
│  ┌──────────────────────────────────────────────┐    │
│  │  nasiko-worker (orchestrator inside Docker)   │    │
│  │  - Listens for deployment commands on Redis   │    │
│  │  - Builds agent images                        │    │
│  │  - Deploys agents via docker-compose          │    │
│  └──────────────────────────────────────────────┘    │
│                                                         │
└─────────────────────────────────────────────────────────┘

All containers connect via internal DNS (service names resolve automatically)
Kong Gateway routes to agents via service discovery
```

## Support & Help

For issues or questions:

1. Check troubleshooting section above
2. View service logs: `nasiko local logs [service]`
3. Check docker-compose.nasiko.yml for service definitions
4. Review environment configuration in `.env`

## Next Steps

- [Deploying Custom Agents](./AGENTS.md) - Guide for creating and deploying your own agents
- [API Reference](./API.md) - Complete REST API documentation
- [Architecture](./ARCHITECTURE.md) - Detailed system design
