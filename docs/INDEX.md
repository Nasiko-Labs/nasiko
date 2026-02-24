# Nasiko Documentation Index

Comprehensive documentation for the Nasiko AI Agent Registry and Orchestration Platform with Kong-powered agent gateway, intelligent routing, and enterprise-grade observability.

## 📚 Core Documentation

| Document | Description | Audience |
|----------|-------------|----------|
| [README.md](../README.md) | Project overview and quick start | Everyone |
| [CLI.md](CLI.md) | Complete CLI reference and command guide | Developers, DevOps |
| [API.md](API.md) | Complete API reference with GitHub integration | Developers, Integrators |
| [ARCHITECTURE.md](ARCHITECTURE.md) | System architecture including Kong and Router | Developers, DevOps |
| [SETUP.md](SETUP.md) | Setup guide for all components | Developers, DevOps |

## 🚀 Getting Started

**New to Nasiko?** Follow this order:
1. [README.md](../README.md) - Platform overview and one-command setup
2. [SETUP.md](SETUP.md) - Detailed installation with Kong, CLI, Router
3. [CLI.md](CLI.md) - Command-line interface for agent management
4. [API.md](API.md) - API endpoints, uploads, GitHub integration
5. [ARCHITECTURE.md](ARCHITECTURE.md) - Complete system design

## ⚡ Quick Access

**Users:** [Quick Start](../README.md#quick-start) | [Kong Gateway](../README.md#agent-access-via-kong)  
**Developers:** [Setup Guide](SETUP.md) | [CLI Reference](CLI.md) | [Architecture](ARCHITECTURE.md)  
**DevOps:** [Production Deployment](SETUP.md#production-deployment) | [Kong Setup](SETUP.md#kong-agent-gateway)

## 🏗️ Platform Components

### Core Services
- **Nasiko Backend** (8000): Agent registry, uploads, management
- **Kong Agent Gateway** (9100): Direct agent access and routing
- **Router Service** (8005): AI-powered query routing
- **Web Frontend** (4000): Flutter-based user interface

### Tools & Integrations
- **CLI Tool**: GitHub auth, repository management, agent deployment
- **Upload System**: ZIP, directory, and GitHub repository uploads
- **Observability**: LangTrace + OpenTelemetry with automatic instrumentation
- **Session Management**: Redis-backed chat sessions and caching

## 🔍 Key Resources

**One-Command Setup:**
```bash
uv run orchestrator/orchestrator.py
```

**Access Points:**
- **Kong Gateway**: http://localhost:9100 (agent access)
- **API Backend**: http://localhost:8000/docs
- **Router Service**: http://localhost:8005
- **LangTrace**: http://localhost:3000

### Platform Features
**Core:** Agent Registry, Kong Gateway, Intelligent Routing, Real-time Observability  
**Agents:** Document Expert, GitHub Agent, Translator, Compliance Checker  
**Upload Methods:** ZIP files, directories, GitHub repositories  
**Tools:** CLI with GitHub integration, automated capabilities generation  

### Technology Stack
**Gateway:** Kong 3.4 with PostgreSQL, service discovery  
**Backend:** FastAPI, MongoDB, Redis, Pydantic  
**AI/ML:** LangChain, OpenAI GPT-4, Anthropic Claude, Google Gemini  
**Infrastructure:** Docker, OTEL Collectors, LangTrace  
**Frontend:** Flutter Web  
**CLI:** Typer with Rich UI

## 📖 Documentation Highlights

### [API.md](API.md) - Enhanced API Reference
- **Agent Upload Endpoints**: ZIP, directory, GitHub integration
- **GitHub OAuth Flow**: Complete authentication workflow
- **Upload Status Tracking**: Real-time deployment monitoring
- **Kong Integration**: Agent routing and gateway endpoints

### [ARCHITECTURE.md](ARCHITECTURE.md) - Complete System Design
- **Kong Agent Gateway**: Service discovery and direct routing
- **Router Service**: AI-powered agent selection logic
- **CLI Tool Architecture**: GitHub integration and command structure
- **Data Flow Diagrams**: Corrected agent communication flows
- **Observability Stack**: Comprehensive monitoring architecture

### [CLI.md](CLI.md) - Complete CLI Reference
- **Installation & Setup**: CLI installation and configuration
- **Authentication**: GitHub OAuth integration and token management
- **Agent Management**: Upload, deploy, and monitor agents
- **Registry Operations**: List, update, and delete registry entries
- **Observability**: Traces, metrics, and performance monitoring
- **Advanced Usage**: Scripting, CI/CD integration, and troubleshooting

### [SETUP.md](SETUP.md) - Comprehensive Setup
- **Kong Configuration**: Gateway setup and management
- **CLI Installation**: GitHub authentication and usage
- **Router Service**: Intelligent routing configuration
- **Upload System**: All upload methods and tracking
- **Troubleshooting**: Kong, Router, and agent discovery issues

## 🎯 Common Workflows

### For End Users
1. **Access Agents**: Use Kong Gateway routes (http://localhost:9100/agent-name/)
2. **Chat Interface**: Web frontend at http://localhost:4000
3. **Monitor Performance**: LangTrace dashboard at http://localhost:3000

### For Developers
1. **Install CLI**: `cd cli && pip install -e .`
2. **GitHub Auth**: `nasiko login`
3. **Start Redis Stream Listener**: `cd orchestrator && uv run redis_stream_listener.py`
4. **Upload Agent**: `nasiko clone user/repo agent-name`
5. **Monitor Deployment**: Check upload status via API

### For DevOps
1. **Production Setup**: Docker, Kubernetes
2. **Kong Management**: Admin API at http://localhost:9101
3. **Observability**: LangTrace + OTEL collectors
4. **Scaling**: Horizontal pod autoscaling configuration

---

**Version:** v1.0.0 | **Support:** [GitHub Issues](https://github.com/arithmic/nasiko/issues) | **Latest Updates:** Kong Gateway, Router Service, CLI Tools, GitHub Integration
