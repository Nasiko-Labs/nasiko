# Nasiko

<div align="center">



**AI Agent Registry and Orchestration Platform**

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Python 3.12+](https://img.shields.io/badge/python-3.12+-blue.svg)](https://www.python.org/downloads/)
[![FastAPI](https://img.shields.io/badge/FastAPI-0.100+-green.svg)](https://fastapi.tiangolo.com/)
[![Kubernetes](https://img.shields.io/badge/kubernetes-1.24+-blue.svg)](https://kubernetes.io/)

**Centralized management, intelligent routing, and observability for AI agents**

[🚀 Quick Start](#-quick-start) •
[📖 Documentation](#-documentation) •
[🏗️ Architecture](#️-architecture) •
[🖥️ Desktop App](#️-desktop-app-macos) •
[🛠️ CLI](#️-cli-tool) •
[☁️ Cloud Setup](#️-cloud-deployment)

</div>

---

## 🌟 What is Nasiko?

Nasiko is a comprehensive platform for managing AI agents at scale. It provides:

- **🔍 Intelligent Agent Discovery** - Automatically route queries to the best AI agent
- **📦 Agent Registry** - Centralized storage and versioning for AI agents
- **🌐 API Gateway** - Unified access point with load balancing and monitoring
- **📊 Observability** - Complete tracing and metrics for agent interactions
- **🚀 Easy Deployment** - One-command agent deployment to Kubernetes or local Docker
- **🔧 Developer Tools** - CLI, SDK, and web interface for seamless integration

## 🚀 Quick Start

### Local Development (Recommended)

```bash
# 1. Clone and setup
git clone https://github.com/your-org/nasiko.git
cd nasiko
cp .env.local.example .env

# 2. Install dependencies
pip install uv
uv sync

# 3. Start the platform
make start-nasiko

# 4. Access the web interface
open http://localhost:4000
```

### Using the CLI Tool

```bash
# Install CLI
cd cli && pip install -e .

# Deploy your first agent
nasiko upload-directory ./agents/document-expert --name doc-agent

# List available agents
nasiko registry-list

# Test agent routing
curl "http://localhost:8005/route?query=analyze this document"
```

## 🖥️ Desktop App (macOS)

### Download and Install

1. **Download the macOS App**:
   ```bash
   # Download the latest release
   curl -L -o nasiko-macos.zip "https://github.com/your-org/nasiko/releases/latest/download/nasiko-macos.zip"
   ```

2. **Extract and Install**:
   ```bash
   # Extract the app
   unzip nasiko-macos.zip
   
   # Move to Applications folder
   mv Nasiko.app /Applications/
   
   # Grant permissions (first time only)
   xattr -dr com.apple.quarantine /Applications/Nasiko.app
   ```

3. **Launch**:
   - Open Finder → Applications → Nasiko
   - Or use Spotlight: `⌘ + Space` → type "Nasiko"

### Desktop App Features

- 🎯 **Native macOS Interface** - Built with SwiftUI for optimal performance
- 🔄 **Real-time Agent Monitoring** - Live status of all deployed agents
- 📊 **Visual Analytics Dashboard** - Interactive charts and metrics
- 🚀 **One-click Agent Deployment** - Drag & drop agent deployment
- 🔍 **Integrated Agent Browser** - Search and discover agents visually
- 💬 **Chat Interface** - Test agents directly from the desktop
- 🔔 **Notifications** - Get alerts for agent failures or completions

## 🏗️ Architecture

Nasiko follows a microservices architecture with these core components:

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│   Web Frontend  │    │   Desktop App   │    │   CLI Tool      │
│   (Port 4000)   │    │   (macOS)       │    │   (Python)     │
└─────────┬───────┘    └─────────┬───────┘    └─────────┬───────┘
          │                      │                      │
          └──────────────────────┼──────────────────────┘
                                 │
                    ┌─────────────▼───────────────┐
                    │     Kong API Gateway       │
                    │       (Port 9100)          │
                    └─────────────┬───────────────┘
                                  │
          ┌───────────────────────┼───────────────────────┐
          │                       │                       │
┌─────────▼───────┐    ┌─────────▼───────┐    ┌─────────▼───────┐
│  FastAPI Backend│    │ Intelligent     │    │   AI Agents     │
│   (Port 8000)   │    │ Router Service  │    │  (Ports 8001+)  │
│                 │    │   (Port 8005)   │    │                 │
│ • Agent Registry│    │ • LangChain     │    │ • Document Expert│
│ • Orchestration │    │ • Query Routing │    │ • GitHub Agent  │
│ • Deployment    │    │ • Capability    │    │ • Translator    │
│ • Monitoring    │    │   Matching      │    │ • Compliance    │
└─────────────────┘    └─────────────────┘    └─────────────────┘
```

### Key Components

- **Kong Gateway** - API routing, load balancing, service discovery
- **FastAPI Backend** - Core platform logic, agent management, orchestration  
- **Router Service** - AI-powered query routing using LangChain
- **Agent Network** - Containerized AI agents with auto-discovery
- **Observability Stack** - LangTrace, OpenTelemetry, distributed tracing

## 🛠️ CLI Tool

The Nasiko CLI provides complete control over your agent ecosystem:

### Installation

```bash
cd cli && pip install -e .
```

### Authentication

```bash
# Login to your Nasiko instance
nasiko login

# Or use environment variables
export NASIKO_API_URL=http://localhost:8000
export NASIKO_API_KEY=your-api-key
```

### Agent Management

```bash
# Upload from directory
nasiko upload-directory ./my-agent --name my-agent

# Upload from Git repository
nasiko clone owner/repo --branch main
nasiko upload-directory ./repo --name repo-agent

# Upload zip file
nasiko upload-zip agent.zip --name packaged-agent

# List deployed agents
nasiko registry-list

# Get agent details
nasiko registry-get --name my-agent

# Update agent metadata
nasiko registry-update agent-123 --description "Updated description"
```

### Monitoring & Debugging

```bash
# Check platform status
nasiko status

# View agent traces
nasiko traces --agent my-agent

# Monitor deployments
nasiko deployment-logs --agent my-agent
```

### Repository Operations

```bash
# List available repositories
nasiko list-repos

# Clone and deploy in one command
nasiko clone-and-deploy owner/repo --name auto-agent
```

## ☁️ Cloud Deployment

Deploy Nasiko to Kubernetes clusters on AWS, DigitalOcean, or other cloud providers:

### Prerequisites

- Kubernetes cluster (1.24+)
- `kubectl` configured
- Container registry access
- OpenAI API key (optional, for router service)

### One-Command Bootstrap

```bash
# DigitalOcean
uv run cli/main.py setup bootstrap \
  --provider digitalocean \
  --registry-name nasiko-images \
  --region nyc3 \
  --openai-key sk-proj-...

# AWS
uv run cli/main.py setup bootstrap \
  --provider aws \
  --registry-name nasiko-images \
  --region us-west-2 \
  --openai-key sk-proj-...
```

This command will:
1. ✅ Provision Kubernetes cluster
2. ✅ Setup container registry  
3. ✅ Deploy BuildKit service
4. ✅ Deploy Nasiko platform
5. ✅ Configure ingress and networking

### Manual Setup Steps

If you prefer step-by-step deployment:

```bash
# 1. Setup Kubernetes cluster
uv run cli/main.py setup k8s deploy --provider digitalocean

# 2. Configure container registry
uv run cli/main.py setup registry deploy --provider digitalocean

# 3. Deploy BuildKit
uv run cli/main.py setup buildkit deploy

# 4. Deploy core platform
uv run cli/main.py setup core deploy
```

### Access Services

After deployment, your services will be available at:

- **Web UI**: `https://nasiko.your-domain.com`
- **API**: `https://api.nasiko.your-domain.com`
- **Kong Gateway**: `https://gateway.nasiko.your-domain.com`
- **LangTrace**: `https://trace.nasiko.your-domain.com`

## 🔧 Local Development

### Prerequisites

- Python 3.12+
- Docker & Docker Compose
- Git

### Detailed Setup

```bash
# 1. Clone repository
git clone https://github.com/your-org/nasiko.git
cd nasiko

# 2. Install UV package manager
pip install uv
uv sync

# 3. Setup environment
cp .env.local.example .env
# Edit .env with your configuration

# 4. Start infrastructure services
make clean-start-nasiko

# 5. Start the Redis stream listener (required for agent uploads)
uv run orchestrator/redis_stream_listener.py
```

### Development Workflow

```bash
# Start everything
make start-nasiko

# View logs
make logs

# Restart specific services
make backend-app     # Restart backend only
make router         # Restart router only

# Clean restart (removes all data)
make clean-all
make start-nasiko

# Stop services
make stop-all
```

### Environment Configuration

Key environment variables:

```bash
# Database
MONGO_URI=mongodb://admin:password@localhost:27017/nasiko
REDIS_HOST=localhost

# API Keys
OPENAI_API_KEY=sk-proj-...
GITHUB_CLIENT_ID=your-github-client-id
GITHUB_CLIENT_SECRET=your-github-client-secret

# Observability
LANGTRACE_BASE_URL=http://localhost:3000

# Kubernetes (for cloud deployment)
BUILDKIT_ADDRESS=tcp://buildkitd.buildkit.svc.cluster.local:1234
REGISTRY_URL=registry.digitalocean.com/nasiko-images
DO_TOKEN=your-digitalocean-token
```

## 📦 Agent Development

### Agent Structure

Every agent must follow this structure:

```
my-agent/
├── Dockerfile              # Container definition
├── docker-compose.yml      # Local development setup
├── capabilities.json       # Agent capabilities (required)
├── src/                    # Source code
│   ├── main.py            # Entry point
│   └── ...                # Your agent logic
├── requirements.txt        # Python dependencies
└── README.md              # Agent documentation
```

### Example Agent

```python
# src/main.py
from fastapi import FastAPI
from pydantic import BaseModel

app = FastAPI()

class QueryRequest(BaseModel):
    query: str

@app.post("/analyze")
async def analyze_query(request: QueryRequest):
    # Your agent logic here
    return {"result": f"Analyzed: {request.query}"}

@app.get("/health")
async def health():
    return {"status": "healthy"}
```

### Capabilities Definition

```json
{
  "name": "document-expert",
  "description": "Expert at analyzing and extracting insights from documents",
  "capabilities": [
    "document_analysis",
    "pdf_extraction", 
    "text_summarization",
    "data_extraction"
  ],
  "input_types": ["text", "pdf", "docx"],
  "output_types": ["json", "text"],
  "endpoints": {
    "/analyze": "Analyze document content",
    "/extract": "Extract structured data",
    "/summarize": "Generate document summary"
  }
}
```

### Testing Agents Locally

```bash
# Deploy locally
nasiko upload-directory ./my-agent --name my-agent

# Test directly
curl -X POST http://localhost:9100/my-agent/analyze \
  -H "Content-Type: application/json" \
  -d '{"query": "test query"}'

# Test via router
curl "http://localhost:8005/route?query=analyze this document" 
```

## 📊 Monitoring & Observability

Nasiko provides comprehensive observability out of the box:

### LangTrace Integration
- **URL**: http://localhost:3000
- Automatic tracing for all agent interactions
- Performance metrics and bottleneck analysis
- Request/response logging with privacy controls

### OpenTelemetry
- Distributed tracing across all services
- Custom metrics collection
- Integration with Prometheus/Grafana

### Kong Analytics
- **URL**: http://localhost:1337
- API usage statistics
- Rate limiting and security metrics
- Service health monitoring

### Built-in Monitoring

```bash
# Check service health
curl http://localhost:8000/health

# View agent registry
curl http://localhost:8000/api/v1/registries

# Check router capabilities
curl http://localhost:8005/capabilities

# Monitor Kong services
curl http://localhost:9101/services
```

## 🔀 Intelligent Routing

The router service automatically selects the best agent for each query:

```python
# Query gets routed to the best agent automatically
response = requests.get(
    "http://localhost:8005/route",
    params={"query": "translate this to French"}
)
# Returns: {"agent_url": "http://localhost:9100/translator", "confidence": 0.95}
```

### How Routing Works

1. **Capability Analysis** - Router analyzes query intent and requirements
2. **Agent Matching** - Compares query with agent capabilities from `capabilities.json`  
3. **Scoring** - Uses LangChain to score agent suitability
4. **Selection** - Returns best matching agent with confidence score
5. **Fallback** - Falls back to general-purpose agents if no specialist found

## 🔌 API Reference

### Core Endpoints

```bash
# Agent Registry
GET    /api/v1/registries           # List agents
POST   /api/v1/registries           # Register agent
GET    /api/v1/registries/{id}      # Get agent
PUT    /api/v1/registries/{id}      # Update agent
DELETE /api/v1/registries/{id}      # Delete agent

# Agent Upload
POST   /api/v1/upload-agents/github     # Upload from GitHub
POST   /api/v1/upload-agents/directory  # Upload from directory

# Query Routing
GET    /api/v1/route                # Route query to best agent
GET    /api/v1/capabilities         # List all capabilities

# Monitoring
GET    /api/v1/health              # Service health
GET    /api/v1/metrics             # Platform metrics
GET    /api/v1/traces/{agent}      # Agent traces
```

Full API documentation: [docs/API.md](docs/API.md)

## 📖 Documentation

- **[Architecture Guide](docs/ARCHITECTURE.md)** - Deep dive into system design
- **[API Reference](docs/API.md)** - Complete REST API documentation
- **[CLI Guide](docs/CLI.md)** - Command-line tool reference
- **[Setup Guide](docs/SETUP.md)** - Detailed setup instructions
- **[Local Development](docs/LOCAL_DEVELOPMENT.md)** - Development environment setup

## 🤝 Contributing

1. Fork the repository
2. Create a feature branch: `git checkout -b feature/amazing-feature`
3. Make your changes and add tests
4. Run the test suite: `make test`
5. Commit your changes: `git commit -m 'Add amazing feature'`
6. Push to the branch: `git push origin feature/amazing-feature`
7. Open a Pull Request


## 🆘 Support

- **Issues**: Report bugs and feature requests on [GitHub Issues](https://github.com/your-org/nasiko/issues)
- **Discussions**: Join the community on [GitHub Discussions](https://github.com/your-org/nasiko/discussions)
- **Documentation**: Comprehensive guides in the [docs/](docs/) directory
