# Nasiko Setup Guide

## 🚀 Quick Start

**Prerequisites:** Python 3.12+, Docker & Docker Compose, Git, UV

```bash
# Clone and setup
git clone git@github.com:arithmic/nasiko.git
cd nasiko
uv sync

# One-command deployment (starts everything!)
uv run orchestrator/orchestrator.py
```

**The orchestrator automatically:**
- Creates Docker networks and deploys infrastructure
- Starts LangTrace (observability), MongoDB, Redis
- Deploys Nasiko API and Flutter web frontend
- Starts Kong agent gateway with service discovery  
- Builds and deploys all agents with observability
- Registers agents with the platform
- Starts Router service for intelligent query routing

## Access Services

- **Nasiko API**: http://localhost:8000/docs
- **Kong Agent Gateway**: http://localhost:9100 (for agent access)
- **Router Service**: http://localhost:8005
- **LangTrace Dashboard**: http://localhost:3000  
- **Web Frontend**: http://localhost:4000
- **Kong Manager**: http://localhost:9102
- **Konga Dashboard**: http://localhost:1337
- **Agents**: Access via Kong routes (recommended)

## Component Overview

### Core Services
- **Nasiko Backend** (Port 8000): Main API with modular repository architecture
  - Registry management for agent metadata
  - Upload status tracking and deployment monitoring
  - Chat session and message history management  
  - GitHub and N8N credentials with encryption
  - Comprehensive agent lifecycle management
- **Router Service** (Port 8005): Intelligent agent selection and routing
- **Web Frontend** (Port 4000): Flutter-based user interface
- **MongoDB** (Port 27017): Primary database with domain-specific collections
- **Redis** (Port 6379): Session management and caching

### Kong Agent Gateway
- **Kong Proxy** (Port 9100): Main gateway for agent access
- **Kong Admin** (Port 9101): Admin API for configuration
- **Kong Manager** (Port 9102): Web-based management interface
- **Konga** (Port 1337): Alternative management dashboard
- **Kong Database**: PostgreSQL backend for Kong configuration
- **Service Registry**: Automatic agent discovery and registration

### Observability Stack
- **LangTrace** (Port 3000): AI-specific tracing and monitoring
- **OTEL Collectors**: Multiple collectors for different components
- **Agent Instrumentation**: Automatic observability injection

### CLI Tool
- **Nasiko CLI**: Build, deploy, and manage AI agents with ease
- **Installation**: `cd cli && pip install -e .`
- **Command Groups**: 
  - Core: `login`, `logout`, `status`, `whoami`, `api-docs`
  - Agent: `agent list`, `agent get`, `agent upload-directory`, `agent upload-zip`
  - GitHub: `github list-repos`, `github clone`
  - Chat: `chat send`, `chat sessions`
  - Search: `search agents`
  - Observability: `observability traces`, `observability summary`
  - Access Control: `access create-token`, `access revoke-token`
  - User Management: `user create`, `user delete`
- **Complete Reference**: See [CLI Documentation](CLI.md) for all commands and examples

## Manual Setup (Alternative)

### 1. Infrastructure Services
```bash
# Start observability
docker-compose -f observability/langtrace/docker-compose.langtrace.yaml up -d

# Start databases and core services
docker-compose -f app/docker-compose.app.yaml up -d

# Start Kong agent gateway
cd kong && docker-compose up -d && cd ..

# Start Router service
cd router && docker-compose up -d && cd ..
```

### 2. Deploy Agents
```bash
# Deploy agents individually
cd agents/document-expert && docker-compose up -d && cd ../..
cd agents/github-agent && docker-compose up -d && cd ../..
cd agents/translator && docker-compose up -d && cd ../..
cd agents/compliance-checker && docker-compose up -d && cd ../..
```

### 3. Start Web Frontend
```bash
cd web && docker-compose up -d && cd ..
```

## CLI Tool Setup

**📖 For complete CLI documentation, see [CLI.md](CLI.md)**

### Installation
```bash
cd cli
pip install -e .
```

### Usage
```bash
# GitHub authentication
nasiko login
nasiko logout

# Repository management
nasiko list-repos
nasiko clone owner/repo-name agent-name

# Status and traces
nasiko status
nasiko traces agent-name
```

## Kong Agent Gateway

### Features
- **Automatic Service Discovery**: Detects agent containers in `agents-net`
- **Dynamic Routing**: Creates routes like `/translator`, `/document-expert`
- **Health Monitoring**: Monitors agent container health
- **Load Balancing**: Distributes requests across agent instances

### Usage
```bash
# Access agents via Kong (recommended)
curl http://localhost:9100/document-expert/chat
curl http://localhost:9100/translator/translate
curl http://localhost:9100/github-agent/analyze

# Monitor Kong services
curl http://localhost:9101/services
curl http://localhost:9101/routes

# Service registry status
curl http://localhost:8080/status
```

## Router Service

### Intelligent Query Routing
```bash
# Send query for automatic agent selection
curl -X POST http://localhost:8005/route \
  -H "Content-Type: application/json" \
  -d '{
    "session_id": "session123",
    "query": "Translate this text to Spanish",
    "has_route": false,
    "route": ""
  }'
```

### Streaming Responses
```bash
# Stream agent selection process
curl -X POST http://localhost:8005/stream \
  -H "Content-Type: multipart/form-data" \
  -F "session_id=session123" \
  -F "query=Analyze this document" \
  -F "has_route=false" \
  -F "route="
```

## Environment Configuration

Create `app/.env` (optional for development):

```bash
# Core Configuration
ENV=development
SECRET_KEY=your-secret-key

# Database
MONGO_NASIKO_HOST=localhost
MONGO_NASIKO_PORT=27017
MONGO_NASIKO_DATABASE=nasiko
MONGO_NASIKO_USER=admin
MONGO_NASIKO_PASSWORD=password

# Redis
REDIS_HOST=localhost
REDIS_PORT=6379
REDIS_PASSWORD=

# Observability  
LANGTRACE_BASE_URL=http://localhost:3000
LANGTRACE_API_KEY=demo-api-key
OTEL_ENDPOINT=http://localhost:4318

# Kong Configuration
KONG_ADMIN_URL=http://localhost:9101
KONG_PROXY_URL=http://localhost:9100

# AI Providers (optional)
OPENAI_API_KEY=your-key
ANTHROPIC_API_KEY=your-key
GOOGLE_AI_API_KEY=your-key

# GitHub Integration
GITHUB_CLIENT_ID=your-github-client-id
GITHUB_CLIENT_SECRET=your-github-client-secret
```

## Production Deployment

### 1. Build Production Images
```bash
# Build main services
docker build -t nasiko:latest .
docker build -t nasiko-router:latest router/
docker build -t nasiko-agents:latest agents/

# Build Kong service registry
cd kong && docker build -t kong-service-registry:latest registry/
```

### 2. Deploy with Docker Swarm
```bash
# Initialize swarm
docker swarm init

# Deploy stack
docker stack deploy -c docker-compose.prod.yml nasiko
```

### 3. Kubernetes Deployment
```bash
# Apply manifests
kubectl apply -f k8s/namespace.yaml
kubectl apply -f k8s/configmap.yaml
kubectl apply -f k8s/secrets.yaml

# Deploy services
kubectl apply -f k8s/mongodb/
kubectl apply -f k8s/redis/
kubectl apply -f k8s/kong/
kubectl apply -f k8s/nasiko/
kubectl apply -f k8s/agents/
```

## Development Setup

### IDE Configuration (VS Code)
```bash
# Install extensions
code --install-extension ms-python.python
code --install-extension ms-python.black-formatter
code --install-extension dart-code.flutter
code --install-extension ms-vscode.docker
```

### Testing
```bash
# Run all tests
pytest

# Test specific components
pytest app/tests/
pytest cli/tests/
pytest router/tests/

# With coverage
pytest --cov=app --cov=cli --cov=router
```

### Pre-commit Hooks
```bash
pip install pre-commit
pre-commit install

# Run manually
pre-commit run --all-files
```

## Agent Upload & Management

**⚠️ Important**: For agent uploads to work, you MUST run the Redis Stream Listener:

```bash
# In a separate terminal window
cd orchestrator
uv run redis_stream_listener.py
```

The Redis Stream Listener processes upload commands asynchronously and handles:
- Docker image building and deployment
- Agent registration with Kong gateway
- Status updates back to the database

Without this service running, uploaded agents will remain in "processing" status.

### Upload Agent Directory
```bash
curl -X POST http://localhost:8000/api/v1/upload-agents/directory \
  -H "Content-Type: application/json" \
  -d '{"directory_path": "/path/to/agent", "agent_name": "my-agent"}'
```

### Upload from GitHub
```bash
curl -X POST http://localhost:8000/api/v1/upload-agents/github \
  -H "Content-Type: application/json" \
  -d '{"repository_url": "https://github.com/user/repo", "agent_name": "github-agent"}'
```

### Upload ZIP File
```bash
curl -X POST http://localhost:8000/api/v1/upload-agents/zip \
  -F "file=@agent.zip" \
  -F "agent_name=my-agent"
```

### Track Upload Status
```bash
# Get all upload statuses
curl http://localhost:8000/api/v1/upload-status

# Get specific upload status
curl http://localhost:8000/api/v1/upload-status/{upload_id}
```

## Troubleshooting

### Port Conflicts
```bash
# Find and kill processes using ports
lsof -i :8000 | grep LISTEN
kill -9 <PID>

# Check all Nasiko ports
lsof -i :8000,:8005,:9100,:3000,:4000
```

### Docker Issues
```bash
# Add user to docker group
sudo usermod -aG docker $USER
newgrp docker

# Reset containers and networks
docker-compose down -v
docker network prune
docker-compose up -d

# Clean up Kong
cd kong && docker-compose down -v && docker-compose up -d
```

### Kong Gateway Issues
```bash
# Check Kong health
curl http://localhost:9100/

# Check Kong admin
curl http://localhost:9101/status

# Check service registry
curl http://localhost:8080/status

# Manually trigger service sync
curl -X POST http://localhost:8080/sync
```

### Agent Discovery Issues
```bash
# Check agents network
docker network ls | grep agents-net

# Check containers in agents network
docker network inspect agents-net

# Check Kong services
curl http://localhost:9101/services

# Check Kong routes
curl http://localhost:9101/routes
```

### Router Service Issues
```bash
# Check router health
curl http://localhost:8005/health

# Check router logs
docker logs router-service

# Test router directly
curl -X POST http://localhost:8005/route \
  -H "Content-Type: application/json" \
  -d '{"session_id": "test", "query": "hello", "has_route": false, "route": ""}'
```

### Health Checks
```bash
# Check all main services
curl http://localhost:8000/api/v1/healthcheck
curl http://localhost:8005/health
curl http://localhost:9100/
curl http://localhost:3000/health

# Check agents via Kong
curl http://localhost:9100/document-expert/health
curl http://localhost:9100/translator/health
```

## Performance Tuning

### Database Optimization
```javascript
// MongoDB indexes
db.registries.createIndex({ "agent.name": 1 })
db.registries.createIndex({ "created_at": -1 })
db.chat_tracks.createIndex({ "agent_name": 1, "created_at": -1 })
db.upload_status.createIndex({ "upload_id": 1 })
db.upload_status.createIndex({ "agent_name": 1, "status": 1 })
```

### Kong Performance
```bash
# Increase Kong worker processes
export KONG_NGINX_WORKER_PROCESSES=4

# Enable Kong caching
export KONG_PROXY_CACHE_SIZE=128m
```

### Production Settings
```bash
# FastAPI with multiple workers
uvicorn app.main:app --workers 4 --host 0.0.0.0 --port 8000

# Router service scaling
docker-compose up --scale router-service=3

# Agent scaling via Kong
docker-compose up --scale document-expert=2 --scale translator=2
```

## Monitoring & Observability

### LangTrace Dashboard
- **URL**: http://localhost:3000
- **Features**: AI-specific tracing, cost monitoring, performance metrics
- **Agent Traces**: Automatic instrumentation for all agents

### Kong Monitoring
- **Kong Manager**: http://localhost:9102
- **Konga Dashboard**: http://localhost:1337
- **Admin API**: http://localhost:9101

### Custom Metrics
```bash
# Agent performance metrics
curl http://localhost:8000/api/v1/get-traces?agent_name=document-expert

# Upload status metrics
curl http://localhost:8000/api/v1/upload-status

# Kong metrics
curl http://localhost:9101/metrics
```

---

**Version:** v1.0.0 | **Support:** [GitHub Issues](https://github.com/arithmic/nasiko/issues)