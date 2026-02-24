# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Nasiko** is an AI agent registry and orchestration platform that provides centralized management, intelligent routing, and observability for AI agents. It consists of a FastAPI backend, Flutter web frontend, Kong API gateway, intelligent router service, and built-in AI agents.

## Architecture

The platform follows a microservices architecture with four main layers:

### 2. Backend Layer (FastAPI)
- **Location**: `/app`
- **Port**: 8000
- **Architecture**: Layered design
  - `api/` - Route handlers (routes.py, handlers.py, types.py)
  - `service/` - Business logic (service.py, k8s_service.py, orchestration_service.py)
  - `repository/` - Data access (repo.py with MongoDB)
  - `entity/` - Pydantic data models (entity.py)
  - `pkg/config/` - Configuration management with Pydantic settings
  - `utils/` - Utilities including capabilities generator

### 3. Router Service
- **Location**: `/router`
- **Port**: 8005
- **Purpose**: AI-powered intelligent query routing using LangChain
- Routes user queries to the most appropriate agent based on capabilities

### 4. Agent Network
- **Location**: `/agents/` (document-expert, github-agent, translator, compliance-checker)
- **Ports**: 8001-8004
- Each agent has: `src/`, `Dockerfile`, `docker-compose.yml`, `capabilities.json`

### 5. Kong Agent Gateway
- **Location**: `/kong`
- **Ports**: 9100 (proxy), 9101 (admin), 9102 (manager UI)
- Auto-discovers agents on Docker network and creates dynamic routes
- **Access agents via**: `http://localhost:9100/<agent-name>/<endpoint>`

### 6. Orchestrator
- **Location**: `/orchestrator`
- Master automation engine that:
  - Sets up Docker networks (`agents-net`, `app-network`)
  - Deploys infrastructure services (MongoDB, Redis, LangTrace, OTEL)
  - Builds and deploys agents with auto-instrumentation
  - Manages Kong gateway and service registry
- **Key Files**:
  - `orchestrator.py` - Main orchestration engine
  - `redis_stream_listener.py` - Async deployment processor (MUST run separately)
  - `agent_builder.py` - Agent build & deploy logic
  - `instrumentation_injector.py` - Auto-injects observability

## Common Commands

### Development Workflow
```bash
# Quick start (recommended)
uv run orchestrator/orchestrator.py

# In separate terminal (REQUIRED for agent uploads)
uv run orchestrator/redis_stream_listener.py

# Install dependencies
uv sync

# Access services
# API: http://localhost:8000/docs
# Kong Gateway: http://localhost:9100
# Router: http://localhost:8005
# LangTrace: http://localhost:3000
# Web: http://localhost:4000
```

### Makefile Targets
```bash
make clean-all           # Stop all containers, remove volumes/images
make clean-start-nasiko  # Full clean and restart
make start-nasiko        # Delete volumes, run orchestrator + redis listener
make backend-app         # Restart backend services
make router              # Restart router service
make orchestrator        # Run orchestrator only
make redis-listener      # Run redis stream listener
```

### CLI Tool (Agent Management)
```bash
# Install CLI
cd cli && pip install -e .

# Authentication
nasiko login

# Repository management
nasiko list-repos
nasiko clone owner/repo [-b branch]

# Agent upload
nasiko upload-directory /path --name agent-name
nasiko upload-zip agent.zip --name agent-name

# Registry operations
nasiko registry-list
nasiko registry-get --name agent-name
nasiko registry-update agent-id --description "text"

# Monitoring
nasiko status
nasiko traces --agent agent-name
```

### Kubernetes/Infrastructure Setup (DigitalOcean/AWS)
```bash
# Location: cli/setup/
# Bootstrap cluster and deploy Nasiko
uv run cli/main.py setup bootstrap --provider digitalocean \
  --registry-name nasiko-images \
  --region nyc3

# Individual setup commands
uv run cli/main.py setup k8s deploy --provider digitalocean
uv run cli/main.py setup registry deploy --provider digitalocean
uv run cli/main.py setup buildkit deploy
uv run cli/main.py setup core deploy
```

## Key Architecture Patterns

### Data Flow Patterns

**Agent Registration Flow**:
1. Client → API (`POST /api/v1/registries`)
2. Service layer validates and processes
3. Repository stores in MongoDB
4. Agent becomes discoverable via Kong Gateway

**Query Routing Flow**:
1. User query → Router Service (port 8005)
2. Router analyzes with LangChain
3. Selects best agent based on capabilities
4. Returns agent URL for client to call

**Agent Upload Flow** (Critical - requires Redis Stream Listener):
1. CLI/Web → API (`POST /api/v1/upload-agents/github` or `/directory`)
2. API stores status in MongoDB, sends command to Redis Stream
3. **Redis Stream Listener** picks up command (separate process)
4. Listener builds Docker image with instrumentation
5. Listener deploys container and registers with API
6. Kong auto-discovers and creates routes

**Observability Flow**:
1. Agents have LangTrace + OpenTelemetry auto-injected during deployment
2. Traces sent to OTEL Collectors (port 4318)
3. Collectors forward to LangTrace API
4. View in LangTrace Dashboard (port 3000)

### Database Strategy
- **MongoDB**: Application data (registries, chat tracking, upload status, builds, deployments)
- **Redis**: Sessions, upload tracking, message streams for async operations
- **PostgreSQL**: Kong configuration storage only

### Configuration Management
- **Root**: `pyproject.toml` - UV package manager, Python 3.12+ dependencies
- **Backend**: `app/pkg/config/config.py` - Pydantic settings with environment loading
- **Environment**: `.env` file for API keys, database credentials, OAuth secrets

Key environment variables:
```
MONGO_URI=mongodb://admin:password@localhost:27017/nasiko
REDIS_HOST=localhost
LANGTRACE_BASE_URL=http://localhost:3000
OPENAI_API_KEY=sk-proj-...
GITHUB_CLIENT_ID/SECRET=OAuth credentials
BUILDKIT_ADDRESS=tcp://buildkitd.buildkit.svc.cluster.local:1234
REGISTRY_URL=registry.digitalocean.com/nasiko-images
DO_TOKEN=DigitalOcean API token (for registry)
```

## Important Implementation Details

### Kubernetes Agent Lifecycle Management

**BuildKit Integration** (`app/service/k8s_service.py`):
- Creates Kubernetes Jobs to build agent images from Git repos
- Uses remote BuildKit daemon in `buildkit` namespace
- Secret name: `agent-registry-credentials` (renamed from `harbor-credentials`)
- Image reference format: `{REGISTRY_URL}/{agent_id}:{version_tag}`

**Key Methods**:
- `create_build_job(job_id, git_url, image_destination)` - Submits K8s Job with git-clone init container + buildctl client
- `deploy_agent(deployment_name, image_reference, port)` - Creates Deployment + Service with imagePullSecrets
- `get_job_status(job_name)` - Returns: active, succeeded, failed, pending, unknown

**Build Job Workflow** (`app/service/service.py`):
1. `trigger_agent_build()` constructs image reference
2. Creates build record in MongoDB (status: QUEUED)
3. Calls `k8s_service.create_build_job()` to submit K8s Job
4. Updates status to BUILDING with K8s job name
5. Job runs: git-clone init container → buildkit-client container builds & pushes
6. Image pushed to registry with read-write credentials

**Deployment Workflow**:
1. `deploy_agent_container()` fetches build record for image reference
2. Creates deployment record (status: STARTING)
3. Calls `k8s_service.deploy_agent()` to create K8s Deployment + Service
4. Updates status to RUNNING with service_url
5. Service URL format: `http://agent-{deploy_id}.nasiko-agents.svc.cluster.local`

### Infrastructure Setup (`cli/setup/`)

**Component Modules**:
- `k8s_setup.py` - Terraform-based cluster provisioning (AWS/DigitalOcean)
- `container_registry_setup.py` - Cloud registry configuration:
  - AWS ECR: Account ID, region, 12-hour token, auto-refresh CronJob
  - DigitalOcean: Uses `doctl registry docker-config --read-write --expiry-seconds 31536000`
- `buildkit_setup.py` - Deploys rootless BuildKit in `buildkit` namespace with PVC for cache
- `app_setup.py` - Deploys core Nasiko apps:
  - Infrastructure: Redis, MongoDB, Kong (via Helm)
  - RBAC: ServiceAccount + Role for backend K8s API access
  - Secrets: `regcred` (nasiko namespace), `agent-registry-credentials` (nasiko-agents namespace)
  - Backend deployment with DO_TOKEN env var for API calls
  - Web, Router, Ollama deployments
  - Kong Ingress rules

**Bootstrap Command** (`cli/main.py`):
```bash
uv run cli/main.py setup bootstrap \
  --provider digitalocean \
  --registry-name nasiko-images \
  --region nyc3 \
  --openai-key sk-...
```
Executes all 4 steps sequentially:
1. Provision K8s cluster (sets KUBECONFIG)
2. Setup container registry
3. Deploy BuildKit service
4. Deploy Nasiko core apps

**Critical Fix Applied**: DigitalOcean registry tokens default to read-only. Must use `--read-write` flag with `doctl registry docker-config` to enable image pushes from BuildKit jobs.

### Network Architecture
- `agents-net` - Internal agent-to-agent communication
- `app-network` - Core services (API, databases, Kong)
- Kong Service Registry auto-discovers containers on `agents-net`

### Kubernetes RBAC
- **File**: `k8s/agent-rbac.yaml`
- **Purpose**: Cross-namespace permissions for backend to manage agents in `nasiko-agents` namespace
- Backend ServiceAccount (`nasiko-backend-sa`) needs ClusterRole to create Jobs, Deployments, Services in agent namespace

## Critical Notes

1. **Redis Stream Listener is Required**: Agent uploads will NOT work without `orchestrator/redis_stream_listener.py` running in a separate process. It processes async deployment commands from Redis streams.

2. **Kong for Agent Access**: Always access agents via Kong Gateway (`http://localhost:9100/<agent-name>`) rather than direct ports. Kong provides service discovery, load balancing, and monitoring.

3. **Capabilities.json is Mandatory**: Every agent MUST have this file defining capabilities. Used for agent discovery, routing decisions, and registry metadata.

4. **API Version Prefix**: All endpoints are under `/api/v1/` prefix.

5. **Automatic Instrumentation**: Orchestrator injects LangTrace + OpenTelemetry into agents during deployment. No manual setup required in agent code.

6. **Docker Networks Must Exist**: `agents-net` and `app-network` must be created before deploying services. Orchestrator handles this automatically.

7. **Orchestrator is Master Controller**: `/orchestrator/orchestrator.py` is the single source of truth for startup flow. Check this file to understand deployment sequence.

8. **BuildKit Secret Name**: Secret is named `agent-registry-credentials` (not `harbor-credentials`). Updated for generic registry support.

9. **Backend Needs DO_TOKEN**: When using DigitalOcean, backend deployment must include `DO_TOKEN` environment variable for API operations (handled by `app_setup.py`).

10. **KUBECONFIG Loading**: Infrastructure setup scripts explicitly load kubeconfig from `KUBECONFIG` environment variable path, not default `~/.kube/config`.

## Documentation

- **Complete Docs**: `/docs/` directory
- **API Reference**: `/docs/API.md` - All REST endpoints
- **CLI Guide**: `/docs/CLI.md` - Complete CLI reference
- **Architecture**: `/docs/ARCHITECTURE.md` - Detailed system design with diagrams
- **Setup Guide**: `/docs/SETUP.md` - Manual setup instructions
- remember to fix the issues in the python code first before fixing the current k8s setup
