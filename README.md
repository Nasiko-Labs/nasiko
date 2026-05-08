# Nasiko

<div align="center">

**AI Agent Developer Control Plane**

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![Python 3.12+](https://img.shields.io/badge/python-3.12+-blue.svg)](https://www.python.org/downloads/)
[![FastAPI](https://img.shields.io/badge/FastAPI-0.100+-green.svg)](https://fastapi.tiangolo.com/)
[![Docker](https://img.shields.io/badge/docker-%230db7ed.svg?style=flat&logo=docker&logoColor=white)](https://docker.com/)

**Centralized management, intelligent routing, and observability for AI agents**

[🚀 Quick Start](#-quick-start) •
[📚 Documentation](docs/) •
[🏗️ Architecture](#️-architecture) •
[🛠️ CLI Tool](#️-cli-tool) •
[📦 Agent Development](#-agent-development)

</div>


---

## 🚀 Track 2 Integration: Platform-Managed LLM Routing
*This fork contains the official Track 2 submission for the Srishti Hackathon.*

Nasiko now features a centralized **LiteLLM Gateway** to manage provider credentials and secure agent routing. This eliminates the critical security risk of hardcoded API keys in agent source code.

**Key Architectural Enhancements:**
* **Centralized Credential Management:** All model provider keys (OpenAI, Anthropic, etc.) are securely stored in the platform-level `litellm_config.yaml`.
* **Zero-Touch Agent Rerouting:** The orchestrator (`agent_builder.py`) now dynamically injects `OPENAI_BASE_URL` and virtual keys into the agent's Docker runtime. 
* **Legacy Compatibility:** Existing agents using standard OpenAI SDKs are automatically hijacked and securely routed through the in-cluster `litellm-gateway:4000` proxy without requiring any developer code changes.
* **Design Documentation:** Full trade-off rationale (LiteLLM vs. Portkey) is available in [`GATEWAY_DESIGN.md`](./GATEWAY_DESIGN.md).

---

## 🌟 What is Nasiko?

Nasiko is a developer control plane that transforms how you build, deploy, and manage AI agents at scale. Built with modern microservices architecture, Nasiko provides everything needed to run production AI agent ecosystems.

### 🎯 Core Capabilities

**Agent Lifecycle Management:**
- **📦 Centralized Registry** - Version-controlled agent storage with metadata management
- **🚀 Automated Deployment** - Docker-based containerization with Kubernetes orchestration
- **📝 AgentCard System** - Structured capability definitions for intelligent routing
- **🔄 Hot Deployment** - Zero-downtime agent updates and rollbacks

**Intelligent Operations:**
- **🧠 LangChain-Powered Routing** - AI-driven query analysis and agent selection
- **⚖️ Load Balancing** - Automatic traffic distribution across agent replicas
- **🎯 Capability Matching** - Semantic matching of queries to agent expertise
- **📊 Confidence Scoring** - Probabilistic agent selection with fallback handling

**Production Infrastructure:**
- **🌐 Kong API Gateway** - Enterprise-grade API management with plugins
- **🔐 Multi-Auth Support** - GitHub OAuth, JWT tokens, and custom authentication
- **💬 Conversation Logging** - Complete chat history and interaction tracking
- **🔍 Service Discovery** - Automatic agent registration and health monitoring

**Developer Experience:**
- **⚡ One-Command Setup** - `docker compose up -d` to full platform
- **🛠️ Rich CLI Tool** - Complete agent management from command line
- **🌐 Web Dashboard** - Browser-based interface accessible via Kong Gateway (/app/)
- **🖥️ Desktop Application** - Native desktop app for enhanced user experience
- **🔗 REST APIs** - Comprehensive programmatic access with OpenAPI docs

**Enterprise Observability:**
- **📈 Integrated Observability Dashboard** - Built-in monitoring within the web UI
- **📋 Request Tracing** - End-to-end visibility across microservices via Arize Phoenix
- **🚨 Health Monitoring** - Automatic agent health checks and alerting
- **📊 Usage Analytics** - Real-time metrics on agent performance and utilization
- **💡 LLM-Native Monitoring** - Specialized observability for AI agent interactions

## 🏗️ Architecture

Nasiko implements a cloud-native microservices architecture designed for enterprise AI agent orchestration:

```
                           ┌─────────────────────────────────────┐
                           │            User Interfaces          │
                           └─────────────────┬───────────────────┘
                                             │
                     ┌─────────────────┬─────────────────┬
                     │                 │                 │                 
                ┌────▼────┐       ┌────▼────┐       ┌────▼────┐
                │Web UI   │       │CLI Tool │       │Desktop  │
                │(/app/)  │       │(Python) │       │App      │
                └─────────┘       └─────────┘       └─────────┘
                │                 │                 │
                └─────────────────┼─────────────────┘
                                  │
                              ┌─────────────▼───────────────┐
                              │      Kong API Gateway       │
                              │         (Port 9100)         │
                              │                             │
                              │ Routes:                     │
                              │ • /agents/{name}/ → Agents │
                              │ • /api/ → Backend API       │
                              │ • /router/ → Router Service │
                              │ • /auth/ → Auth Service     │
                              │ • /app/ → Web Interface     │
                              │ • /n8n/ → N8N Workflows     │
                              │ • / → Landing (→ /app/)     │
                              └─────────────┬───────────────┘
                                            │
              ┌─────────────────────────────┼─────────────────────────────┐
              │                             │                             │
    ┌─────────▼─────────┐         ┌─────────▼─────────┐         ┌─────────▼─────────┐
    │   Core Platform   │         │  Intelligence     │         │    AI Agents      │
    │    Services       │         │    Services       │         │   (Dynamic)       │
    └─────────┬─────────┘         └─────────┬─────────┘         └─────────┬─────────┘
              │                             │                             │
    ┌─────────▼─────────┐         ┌─────────▼─────────┐         ┌─────────▼─────────┐
    │FastAPI Backend    │         │Router Service     │         │compliance-checker │
    │Port: 8000         │         │Port: 8081         │         │github-agent      │
    │                   │         │                   │         │translator         │
    │• Agent Registry   │         │• LangChain Engine │         │crewai-workflows   │
    │• Upload System    │         │• Query Analysis   │         │langgraph-flows    │
    │• Kubernetes Orch. │         │• Capability Match │         │custom-agents      │
    │• GitHub OAuth     │         │• Confidence Score │         │... (auto-deployed)│
    │• Build Pipeline   │         │• Fallback Logic   │         │                   │
    │• Health Monitoring│         │• Model Selection  │         │• Health Endpoints │
    └───────────────────┘         └───────────────────┘         │• Auto-Scaling     │
              │                             │                   │• Phoenix Tracing  │
              │                             │                   └───────────────────┘
    ┌─────────▼─────────┐         ┌─────────▼─────────┐                   │
    │Auth Service       │         │Chat History       │                   │
    │Port: 8082         │         │Port: 8083         │                   │
    │                   │         │                   │                   │
    │• JWT Management   │         │• Conversation Log │                   │
    │• GitHub OAuth     │         │• Chat Persistence │                   │
    │• User Sessions    │         │• Retrieval APIs   │                   │
    │• Role-Based Auth  │         │• Search & Filter  │                   │
    └───────────────────┘         └───────────────────┘                   │
              │                             │                             │
    ┌─────────▼─────────┐                   │                             │
    │Kong Registry      │                   │                             │
    │Port: 8080         │                   │                             │
    │                   │                   │                             │
    │• Service Discovery│                   │                             │
    │• Auto-Registration│                   │                             │
    │• Health Checks    │                   │                             │
    │• Route Management │                   │                             │
    └───────────────────┘                   │                             │
              │                             │                             │
              └─────────────────────────────┼─────────────────────────────┘
                                            │
                              ┌─────────────▼───────────────┐
                              │     Infrastructure &        │
                              │      Observability          │
                              └─────────────┬───────────────┘
                                            │
        ┌─────────────────┬─────────────────┼─────────────────┬─────────────────┐
        │                 │                 │                 │                 │
   ┌────▼────┐       ┌────▼────┐       ┌────▼────┐       ┌────▼────┐       ┌────▼────┐
   │MongoDB  │       │Redis    │       │Phoenix  │       │Kong DB  │       │BuildKit │
   │:27017   │       │:6379    │       │:6006    │       │(PostgSQL│       │(K8s)    │
   │         │       │         │       │         │       │:5432)   │       │         │
   │• Agent  │       │• Session│       │• LLM    │       │• Gateway│       │• Image  │
   │  Storage│       │  Cache  │       │  Traces │       │  Config │       │  Builds │
   │• Users  │       │• Queues │       │• Request│       │• Routes │       │• Multi- │
   │• Chat   │       │• Pub/Sub│       │  Flows  │       │• Plugins│       │  Arch   │
   │  History│       │• Locks  │       │• Metrics│       │• Rate   │       │• Registry│
   └─────────┘       └─────────┘       └─────────┘       │  Limits │       │  Push   │
                                                         └─────────┘       └─────────┘
```

### 🔄 Data Flow Patterns

**Agent Deployment Flow:**
```
CLI/Web → Backend API → Redis Stream → Build System → Container Registry → K8s Deployment → Kong Registration
```

**Query Routing Flow:**  
```  
User Query → Kong Gateway → Router Service → LangChain Analysis → Agent Selection → Kong Proxy → Agent Response
```

**Observability Flow:**
```
Agent Request → Phoenix SDK → Trace Collection → Nasiko Web UI + Phoenix Dashboard → Performance Analytics
```

### Supported LLM Providers

Nasiko supports multiple LLM providers for both the routing engine and agent execution:

| Provider | API Key Env Var | Base URL | Models |
|----------|----------------|----------|--------|
| OpenAI (default) | `OPENAI_API_KEY` | Default OpenAI endpoint | gpt-4o, gpt-4o-mini |
| [MiniMax](https://platform.minimax.io) | `MINIMAX_API_KEY` | `https://api.minimax.io/v1` (global) / `https://api.minimaxi.com/v1` (China) | MiniMax-M2.7, MiniMax-M2.7-highspeed, MiniMax-M2.5, MiniMax-M2.5-highspeed |
| OpenRouter | `OPENROUTER_API_KEY` | `https://openrouter.ai/api/v1` | Various models |

To switch the router's LLM provider, set `ROUTER_LLM_PROVIDER` and `ROUTER_LLM_MODEL` in your environment configuration. See [Environment Configuration](#-environment-configuration).

### Key Components

- **Kong Gateway** (9100) - API routing, load balancing, service discovery
- **FastAPI Backend** (8000) - Agent registry, orchestration, agent upload system
- **Auth Service** (8082) - User authentication, GitHub OAuth, JWT token management
- **Router Service** (8081) - LangChain-powered intelligent query routing
- **Chat History** (8083) - Conversation logging and retrieval service
- **Kong Registry** (8080) - Automatic agent service discovery and registration
- **Web Interface** (4000) - Browser dashboard accessible via Kong Gateway (/app/)
- **Agent Network** - Auto-deployed containerized agents with observability
- **CLI Tool** - Complete command-line management interface

## 🚀 Quick Start

### Prerequisites

- Docker & Docker Compose
- Python 3.12+
- 4GB+ RAM recommended

### Local Development Setup

```bash
# 1. Clone the repository
git clone https://github.com/Nasiko-Labs/nasiko.git
cd nasiko

# 2. Create environment configuration
cp .nasiko-local.env.example .nasiko-local.env

# 3. Edit .nasiko-local.env with your API keys:
# Generate a secure base64-encoded encryption key
# Example: 5kfdxaT7WRoseTKqksUY4gR2idR4FuBBEIQk5Cpzlek=
# USER_CREDENTIALS_ENCRYPTION_KEY=5kfdxaT7WRoseTKqksUY4gR2idR4FuBBEIQk5Cpzlek=

# (optional but recommended)
# OPENAI_API_KEY=<sk-your-openai-key>
# GITHUB_CLIENT_ID=<your-github-oauth-id>
# GITHUB_CLIENT_SECRET=<your-github-oauth-secret>

# 4. Install Python dependencies (for CLI)
pip install uv
uv sync

# 5. Start the entire platform
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env up -d

# 6. Access the web interface via Kong Gateway
open http://localhost:9100/app/
```

### Verify Installation

```bash
# Check all services are healthy
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env ps

# Test the API
curl http://localhost:8000/api/v1/healthcheck

# Test Kong gateway
curl http://localhost:9100/health
```

**🎉 Success!** Access Nasiko at http://localhost:9100/app/

## 📚 Documentation

For comprehensive guides and detailed instructions:

- **[Getting Started Guide](docs/getting-started.md)** - First login, deploying your first agent, and testing it 
- **[API Reference](http://localhost:8000/docs)** - Full REST API documentation (after startup)

### Quick Links

- **📖 First Login & Agent Deploy**: After setup, follow the [Getting Started Guide](docs/getting-started.md) to sign in and deploy your first agent
- **🔑 Login Credentials**: Generated automatically at `orchestrator/superuser_credentials.json`
- **🤖 Test Agent**: Use `agents/a2a-translator.zip` for your first agent upload

## 🛠️ CLI Tool

The Nasiko CLI provides complete platform management:

### Installation & Authentication

```bash
# Install CLI (uv sync at the repo root installs all dependencies including the CLI)
pip install uv
uv sync

# Or install the CLI standalone
# cd cli && pip install -e .

# Configure API endpoint
export NASIKO_API_URL=http://localhost:9100

# Authenticate with your access key and secret
# a superuser is automatically created during setup and can be found at nasiko/orchestrator/superuser_credentials.json.
nasiko login

# Check status
nasiko status
```

### Agent Management

```bash
# Upload agent from directory
nasiko agent upload-directory ./my-agent --name my-agent

# Upload from GitHub repository (clone and upload in one step)
nasiko github clone owner/repo --branch main

# Upload ZIP file
nasiko agent upload-zip agent.zip --name packaged-agent

# Manage registry
nasiko agent list
nasiko agent get --name my-agent
```

### Monitoring & Operations

```bash
# Platform monitoring
nasiko status
nasiko observability sessions

# Repository operations
nasiko github repos
nasiko github clone owner/repo --branch feature-branch

# Infrastructure (K8s)
nasiko setup bootstrap --provider digitalocean --region nyc3
```

## 📦 Agent Development

### Agent Structure

Every agent must follow this structure:

```
my-agent/
├── AgentCard.json          # Required: Agent capabilities
├── Dockerfile              # Container definition
├── pyproject.toml          # Python dependencies
├── docker-compose.yml      # Local development (optional)
├── src/                    # Source code
│   ├── main.py            # FastAPI entry point
│   └── ...                # Agent logic
└── README.md              # Documentation
```

### Example Agent

**AgentCard.json** (Required):
```json
{
  "name": "document-analyzer",
  "description": "AI agent for document analysis and extraction",
  "capabilities": [
    "document_analysis",
    "pdf_extraction", 
    "text_summarization"
  ],
  "tags": ["nlp", "documents", "analysis"],
  "examples": [
    "analyze this contract",
    "extract data from PDF",
    "summarize document"
  ],
  "input_mode": "text",
  "output_mode": "json",
  "agent_protocol_version": "a2a-v1",
  "endpoints": {
    "/analyze": "Analyze document content",
    "/extract": "Extract structured data",
    "/health": "Health check endpoint"
  }
}
```

**src/main.py**:
```python
from fastapi import FastAPI
from pydantic import BaseModel

app = FastAPI()

class AnalysisRequest(BaseModel):
    text: str
    options: dict = {}

@app.post("/analyze")
async def analyze_document(request: AnalysisRequest):
    # Your agent logic here
    return {
        "summary": f"Analysis of: {request.text[:100]}...",
        "entities": ["entity1", "entity2"],
        "sentiment": "neutral"
    }

@app.get("/health")
async def health_check():
    return {"status": "healthy", "service": "document-analyzer"}
```

**Dockerfile**:
```dockerfile
FROM python:3.12-slim

WORKDIR /app
# Your agent's pyproject.toml should list its own dependencies (fastapi, uvicorn, etc.)
# See agents/a2a-translator/ for a working example.
COPY pyproject.toml .
RUN pip install -e .

COPY src/ ./src/
EXPOSE 8000

CMD ["uvicorn", "src.main:app", "--host", "0.0.0.0", "--port", "8000"]
```

### Testing Agents Locally

```bash
# Test agent directly
cd my-agent
docker compose up -d

# Deploy to Nasiko
nasiko agent upload-directory . --name my-agent

# Test via Kong gateway
curl -X POST http://localhost:9100/agents/my-agent/analyze \
  -H "Content-Type: application/json" \
  -d '{"text": "Sample document content"}'

# Test via intelligent routing through Kong
curl "http://localhost:9100/router/route?query=analyze this document"
```

## 🔄 Intelligent Routing System

The router service automatically selects the best agent for each query:

### How It Works

1. **Query Analysis** - LangChain analyzes user intent and requirements
2. **Capability Matching** - Compares query against AgentCard.json capabilities  
3. **Confidence Scoring** - Ranks agents by suitability
4. **Best Match Selection** - Returns optimal agent URL with confidence score

### Usage Examples

```bash
# Router automatically selects best agent
curl "http://localhost:9100/router/route?query=translate this to French"
# Returns: {"agent_url": "http://localhost:9100/agents/translator", "confidence": 0.95}

curl "http://localhost:9100/router/route?query=check code compliance"  
# Returns: {"agent_url": "http://localhost:9100/agents/compliance-checker", "confidence": 0.89}

# Fallback handling
curl "http://localhost:9100/router/route?query=unknown task"
# Returns: {"agent_url": "http://localhost:9100/agents/general-agent", "confidence": 0.45}
```

## 📊 Observability & Monitoring

### Automatic Instrumentation

All agents automatically receive:
- **Arize Phoenix SDK** injection for LLM observability
- **Automatic instrumentation** for request/response tracing  
- **Chat logging** via Kong plugins with conversation persistence
- **Health monitoring** with automatic restarts and failover

### Monitoring Dashboards

- **Nasiko Web UI**: http://localhost:9100/app/ - Integrated observability dashboard via Kong Gateway
- **Arize Phoenix UI**: http://localhost:6006 - Direct access to detailed traces and performance metrics
- **Kong Manager**: http://localhost:9102 - API gateway analytics and configuration
- **Agent Registry**: http://localhost:8000/docs - REST API documentation and testing

### Health Checks

```bash
# Service health
curl http://localhost:8000/api/v1/healthcheck   # Backend
curl http://localhost:8081/health               # Router
curl http://localhost:9100/health               # Kong

# Agent health via Kong Gateway
curl http://localhost:9100/agents/my-agent/health

# Comprehensive status
nasiko status
```

## 🌐 Environment Configuration

### Required Environment Variables

```bash
# .nasiko-local.env

# API Keys (Optional but recommended)
OPENAI_API_KEY=<sk-your-openai-api-key>
MINIMAX_API_KEY=<your-minimax-api-key>
GITHUB_CLIENT_ID=<your-github-oauth-client-id>
GITHUB_CLIENT_SECRET=<your-github-oauth-secret>

# Router LLM Provider (Optional - defaults to openai)
# Supported: "openai", "openrouter", "minimax"
# ROUTER_LLM_PROVIDER=openai
# ROUTER_LLM_MODEL=gpt-4o-mini

# Security (Change in production)
JWT_SECRET=<your-jwt-signing-secret>
USER_CREDENTIALS_ENCRYPTION_KEY=<base64-encoded-key>

# Database Credentials
MONGO_ROOT_PASSWORD=secure-mongo-password
KONG_DB_PASSWORD=secure-kong-password

# Default Admin Account
SUPERUSER_EMAIL=admin@example.com
SUPERUSER_USERNAME=admin
SUPERUSER_PASSWORD=changeme123

# Port Configuration (Optional - defaults shown)
NASIKO_PORT_BACKEND=8000
NASIKO_PORT_WEB=4000
NASIKO_PORT_KONG=9100
NASIKO_PORT_ROUTER=8081
NASIKO_PORT_PHOENIX=6006
```

### Service Ports

| Service | Port | Purpose |
|---------|------|---------|
| Web Interface | 4000 | Browser dashboard (access via Kong Gateway at /app/) |
| Backend API | 8000 | REST API and documentation |
| Auth Service | 8082 | User authentication and GitHub OAuth |
| Router Service | 8081 | Intelligent query routing |
| Chat History | 8083 | Conversation logging and retrieval |
| Kong Gateway | 9100 | Agent access point |
| Kong Admin | 9101 | Gateway configuration |
| Kong Manager | 9102 | Gateway web UI |
| Kong Registry | 8080 | Service discovery and registration |
| Arize Phoenix | 6006 | Observability and LLM tracing |
| MongoDB | 27017 | Primary database |
| Redis | 6379 | Caching and sessions |

## ☁️ Production Deployment

### Cloud Setup (One Command)

```bash
# DigitalOcean Kubernetes
uv run cli/main.py setup bootstrap \
  --provider digitalocean \
  --registry-name nasiko-images \
  --region nyc3 \
  --openai-key sk-proj-your-key

# AWS Kubernetes  
uv run cli/main.py setup bootstrap \
  --provider aws \
  --registry-name nasiko-images \
  --region us-west-2 \
  --openai-key sk-proj-your-key
```

This command automatically:
1. ✅ Provisions Kubernetes cluster with Terraform
2. ✅ Sets up container registry with credentials
3. ✅ Deploys BuildKit for remote image building
4. ✅ Installs Nasiko platform with Helm
5. ✅ Configures ingress and networking

### Manual Setup Steps

```bash
# 1. Provision cluster
uv run cli/main.py setup k8s deploy --provider digitalocean

# 2. Configure registry
uv run cli/main.py setup registry deploy --provider digitalocean

# 3. Deploy BuildKit
uv run cli/main.py setup buildkit deploy

# 4. Deploy core platform
uv run cli/main.py setup core deploy
```

### Production Architecture

- **Load Balancing**: Kong gateway with multiple replicas
- **Auto-scaling**: Kubernetes HPA for agents
- **Storage**: Persistent volumes for databases
- **Registry**: Cloud container registries (ECR, DigitalOcean)
- **Building**: Remote BuildKit with registry integration
- **Monitoring**: Arize Phoenix + cloud observability

## 📚 Sample Agents

Nasiko includes several example agents:

### Available Agents

- **`agents/a2a-compliance-checker/`** - Document policy compliance analysis
- **`agents/a2a-github-agent/`** - GitHub repository operations
- **`agents/a2a-translator/`** - Multi-language translation service

### Deploy Sample Agents

```bash
# Deploy compliance checker
nasiko agent upload-directory ./agents/a2a-compliance-checker --name compliance

# Deploy GitHub agent
nasiko agent upload-directory ./agents/a2a-github-agent --name github

# Test deployed agents via Kong Gateway
curl "http://localhost:9100/router/route?query=check document compliance"
curl "http://localhost:9100/router/route?query=create GitHub issue"
```

## 🔧 Development Workflow

### Local Development Commands

```bash
# Start all services
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env up -d

# View logs
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env logs -f

# Restart specific services
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env restart nasiko-backend
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env restart nasiko-router

# Stop all services
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env down

# Clean restart (removes data)
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env down -v
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env up -d
```

### Alternative Makefile Commands

The Makefile provides an alternative workflow for running orchestrator components directly on the host (outside Docker) using `uv`. This is useful when iterating on orchestrator code without rebuilding containers.

```bash
make start-nasiko        # Clean volumes + run orchestrator and redis listener on host
make orchestrator        # Run orchestrator only (via uv)
make redis-listener      # Run Redis stream processor (via uv)
make clean-all          # Nuclear cleanup — stops all containers, removes volumes and images
make backend-app        # Restart backend services
```

## 🚨 Important Notes

### Critical Dependencies

1. **Redis Stream Listener** - Agent uploads are processed by the `nasiko-redis-listener` service, which starts automatically with Docker Compose. If agent uploads are failing, check that it's healthy:

   ```bash
   docker logs nasiko-redis-listener
   docker compose -f docker-compose.local.yml --env-file .nasiko-local.env restart nasiko-redis-listener
   ```

2. **Docker Networks** - Required networks created automatically:
   - `app-network` - Core services communication
   - `agents-net` - Agent-to-agent communication

3. **AgentCard.json** - Mandatory for all agents, defines capabilities for routing

4. **BuildKit** - Required for Kubernetes agent deployments

### Access Patterns

**Kong Gateway Routes** (http://localhost:9100):
- **`/agents/{agent-name}/`** - Dynamic agent access (auto-registered)
- **`/api/`** - Backend API with authentication
- **`/router/`** - Intelligent query routing service  
- **`/auth/`** - Authentication endpoints
- **`/app/`** - Web application interface
- **`/n8n/`** - N8N workflow automation
- **`/`** - Landing page (redirects to /app/)

**Direct Service Access** (for development only):
- **Backend API**: `http://localhost:8000/api/v1/`
- **Web Interface**: `http://localhost:4000` (use Kong Gateway `/app/` for production)
- **Router Service**: `http://localhost:8081` (use Kong Gateway `/router` for production)

## 🔍 Troubleshooting

### Common Issues

**Agent won't deploy:**
```bash
# Check Redis stream listener is running
docker logs nasiko-redis-listener

# Restart the listener if needed
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env restart nasiko-redis-listener

# Check Docker daemon
docker info

# Check logs
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env logs nasiko-backend
```

**Connection refused:**
```bash
# Check services are running
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env ps

# Check ports
lsof -i :8000
lsof -i :9100

# Restart services
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env restart
```

**Routing not working:**
```bash
# Verify router service
curl http://localhost:8081/health

# Check agent registration
curl http://localhost:8000/api/v1/registries

# Verify AgentCard.json exists in agent directory
```

## 🤝 Contributing

1. Fork the repository
2. Create a feature branch: `git checkout -b feature/amazing-feature`
3. Make your changes
4. Test locally: `docker compose -f docker-compose.local.yml --env-file .nasiko-local.env up -d`
5. Commit changes: `git commit -m 'Add amazing feature'`
6. Push to branch: `git push origin feature/amazing-feature`
7. Open a Pull Request

## 📄 License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.

## 🆘 Support

- **Issues**: [GitHub Issues](https://github.com/Nasiko-Labs/nasiko/issues)
- **Discussions**: [GitHub Discussions](https://github.com/Nasiko-Labs/nasiko/discussions)
- **Documentation**: This README covers the complete system

---

<div align="center">
<strong>Built with ❤️ for the AI agent community</strong>
</div>
