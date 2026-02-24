# Nasiko Architecture Documentation

## System Overview

Nasiko is an AI agent registry and orchestration platform designed for scalability, observability, and extensibility. The system follows a layered architecture pattern with clear separation of concerns and features a modular repository architecture for efficient data management.

## High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        Docker Host Environment                          │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │                      Core Services                              │    │
│  │  ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐    │    │
│  │  │  Web Frontend   │ │ Nasiko Backend  │ │  Router Service │    │    │
│  │  │   (Flutter)     │ │   (FastAPI)     │ │   (FastAPI)     │    │    │
│  │  │   Port: 4000    │ │   Port: 8000    │ │   Port: 8005    │    │    │
│  │  └─────────────────┘ └─────────────────┘ └─────────────────┘    │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│                                     │                                   │
│                                     ▼                                   │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │                   Kong Agent Gateway                            │    │
│  │  ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐    │    │
│  │  │   Kong Proxy    │ │  Kong Admin     │ │  Kong Manager   │    │    │
│  │  │   Port: 9100    │ │  Port: 9101     │ │  Port: 9102     │    │    │
│  │  └─────────────────┘ └─────────────────┘ └─────────────────┘    │    │
│  │  ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐    │    │
│  │  │Service Registry │ │     Konga       │ │ Kong Database   │    │    │
│  │  │  Port: 8080     │ │  Port: 1337     │ │  (PostgreSQL)   │    │    │
│  │  └─────────────────┘ └─────────────────┘ └─────────────────┘    │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│                                     │                                   │
│                                     ▼                                   │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │                    Agent Containers                             │    │
│  │  ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐    │    │
│  │  │ Document Expert │ │ GitHub Agent    │ │   Translator    │    │    │
│  │  │   Port: 8001    │ │   Port: 8002    │ │   Port: 8003    │    │    │
│  │  │ + Observability │ │ + Observability │ │ + Observability │    │    │
│  │  └─────────────────┘ └─────────────────┘ └─────────────────┘    │    │
│  │  ┌─────────────────┐                                            │    │
│  │  │Compliance Check │                                            │    │
│  │  │   Port: 8004    │                                            │    │
│  │  │ + Observability │                                            │    │
│  │  └─────────────────┘                                            │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│                                     │                                   │
│                                     ▼                                   │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │                Infrastructure Containers                        │    │
│  │  ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐    │    │
│  │  │    MongoDB      │ │     Redis       │ │   LangTrace     │    │    │
│  │  │  Port: 27017    │ │  Port: 6379     │ │  Port: 3000     │    │    │
│  │  │ Agent Registry  │ │Session & Cache  │ │ AI Observability│    │    │
│  │  │ Chat Tracking   │ │Upload Tracking  │ │ Cost Monitoring │    │    │
│  │  │ Upload Status   │ │Stream Messaging │ │ Performance     │    │    │
│  │  └─────────────────┘ └─────────────────┘ └─────────────────┘    │    │
│  │  ┌─────────────────┐                                            │    │
│  │  │ OTEL Collectors │                                            │    │
│  │  │  Port: 4318     │                                            │    │
│  │  │Multi-Service    │                                            │    │
│  │  │ Tracing         │                                            │    │
│  │  └─────────────────┘                                            │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│                                                                         │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │                     Docker Networks                             │    │
│  │  • agents-net: Agent container communication                    │    │
│  │  • app-network: Core service communication                      │    │
│  │  • kong-net: Kong gateway internal communication                │    │
│  └─────────────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────────────┘

External Access (Host Ports):
• Web Frontend: http://localhost:4000
• Nasiko API: http://localhost:8000/docs  
• Router Service: http://localhost:8005
• Kong Gateway: http://localhost:9100 (agent access)
• Kong Admin: http://localhost:9101
• Kong Manager: http://localhost:9102
• LangTrace: http://localhost:3000
• Konga: http://localhost:1337

CLI Tool: Installed locally, connects to APIs above - [Complete CLI Reference](CLI.md)
• Organized command groups: agent, github, chat, n8n, search, observability, access, user
• Interactive commands with prompts for ease of use
• Comprehensive authentication and authorization support
```

## CLI Architecture

### Command Organization

The Nasiko CLI is built using Typer and organized into logical command groups for better usability:

```
┌─────────────────────────────────────────────────────────────────┐
│                        Nasiko CLI                               │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                   Core Commands                           │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐  │  │
│  │  │   login     │ │   logout    │ │      status         │  │  │
│  │  │   whoami    │ │    docs     │ │                     │  │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘  │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                │                                │
│                                ▼                                │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                Command Groups                             │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐  │  │
│  │  │   agent     │ │   github    │ │       chat          │  │  │
│  │  │ • list      │ │ • login     │ │ • create-session    │  │  │
│  │  │ • get       │ │ • repos     │ │ • list-sessions     │  │  │
│  │  │ • upload-*  │ │ • clone     │ │ • send              │  │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘  │  │
│  │                                                           │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐  │  │
│  │  │    n8n      │ │   search    │ │   observability     │  │  │
│  │  │ • register  │ │ • users     │ │ • traces            │  │  │
│  │  │ • connect   │ │ • agents    │ │ • summary           │  │  │
│  │  │ • workflows │ │             │ │                     │  │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘  │  │
│  │                                                           │  │
│  │  ┌─────────────┐ ┌─────────────┐                         │  │
│  │  │   access    │ │    user     │                         │  │
│  │  │ • grant-*   │ │ • register  │                         │  │
│  │  │ • revoke-*  │ │ • list      │                         │  │
│  │  │ • list      │ │ • delete    │                         │  │
│  │  └─────────────┘ └─────────────┘                         │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                │                                │
│                                ▼                                │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                   Interactive Features                    │  │
│  │  • Prompts for required parameters                       │  │
│  │  • Contextual help and autocomplete                      │  │
│  │  • Progress indicators for long operations               │  │
│  │  • Formatted output (table, JSON, details)              │  │
│  │  • Error handling and user-friendly messages            │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

### CLI Authentication Flow

```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│   User      │    │   CLI       │    │ Nasiko API  │    │   GitHub    │
│             │    │   Tool      │    │  Backend    │    │   OAuth     │
└──────┬──────┘    └──────┬──────┘    └──────┬──────┘    └──────┬──────┘
       │                  │                  │                  │
       │ nasiko login     │                  │                  │
       ├─────────────────►│                  │                  │
       │                  │                  │                  │
       │                  │ GET /auth/github/login              │
       │                  ├─────────────────►│                  │
       │                  │                  │                  │
       │                  │                  │ Redirect to OAuth│
       │                  │                  ├─────────────────►│
       │                  │                  │                  │
       │ Browser Opens    │                  │                  │
       │◄─────────────────┤                  │                  │
       │                  │                  │                  │
       │ OAuth Consent    │                  │                  │
       ├─────────────────────────────────────┼─────────────────►│
       │                  │                  │                  │
       │                  │                  │ OAuth Callback   │
       │                  │                  │◄─────────────────┤
       │                  │                  │                  │
       │                  │ Token Stored     │                  │
       │                  │◄─────────────────┤                  │
       │                  │                  │                  │
       │ Authentication   │                  │                  │
       │ Success          │                  │                  │
       │◄─────────────────┤                  │                  │
       │                  │                  │                  │
```

## Component Architecture

### 1. Frontend Layer (Web)

```
┌─────────────────────────────────────────────────────────────────┐
│                        Flutter Web Frontend                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐              │
│  │   Widgets   │  │   Services  │  │   Models    │              │
│  │   (UI)      │  │ (API Calls) │  │  (Data)     │              │
│  └─────────────┘  └─────────────┘  └─────────────┘              │
│                                                                 │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐              │
│  │    State    │  │   Router    │  │   Utils     │              │
│  │ Management  │  │   (Pages)   │  │ (Helpers)   │              │
│  └─────────────┘  └─────────────┘  └─────────────┘              │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼ HTTP/WebSocket
┌─────────────────────────────────────────────────────────────────┐
│                         FastAPI Backend                         │
└─────────────────────────────────────────────────────────────────┘
```

### 2. Backend Layer (FastAPI Application)

```
┌─────────────────────────────────────────────────────────────────-┐
│                         FastAPI Application                      │
├─────────────────────────────────────────────────────────────────-┤
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │                 Organized API Layer                        │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐   │  │
│  │  │Registry     │ │Agent Upload │ │  Chat & Session     │   │  │
│  │  │Routes       │ │Routes       │ │    Routes           │   │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘   │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐   │  │
│  │  │GitHub &     │ │Search &     │ │   Superuser &       │   │  │
│  │  │N8N Routes   │ │Health Routes│ │   Traces Routes     │   │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘   │  │
│  │                                                            │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                │                                 │
│                                ▼                                 │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │                   Service Layer                            │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐   │  │
│  │  │   Registry  │ │Upload Status│ │  GitHub & N8N       │   │  │
│  │  │   Service   │ │  Service    │ │  Services           │   │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘   │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐   │  │
│  │  │Chat Session │ │Search &     │ │   Traces & Auth     │   │  │
│  │  │Service      │ │Agent Service│ │     Services        │   │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘   │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                │                                 │
│                                ▼                                 │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │               Modular Repository Layer                     │  │
│  │  ┌─────────────--┐  ┌─────────────-┐ ┌─────────────────--┐ │  │
│  │  │Base Repository|  │ Registry Repo│ |                   │ │  |
│  │  │(Encryption)   │  │(Agent Mgmt)  │ │ Upload Status Repo│ │  │
│  │  └────────────--─┘  └─────────────-┘ │ (Deployment Track)│ │  │
│  │  ┌─────────────┐ ┌─────────────┐     └───────────────────┘ │  │
│  │  │ Chat Repo   │ │N8N & GitHub │ ┌─────────────────────┐   │  │
│  │  │(Sessions)   │ │(Credentials)│ │ Main Repository     │   │  │
│  │  └─────────────┘ └─────────────┘ │ (Composition)       │   │  │
│  │                                  └─────────────────────┘   │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                │                                 │
│                                ▼                                 │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │                    Entity Layer                            │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐   │  │
│  │  │   Registry  │ │ Chat Track  │ │      Common         │   │  │
│  │  │   Entities  │ │  Entities   │ │     Entities        │   │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘   │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                │                                 │
│                                ▼                                 │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │                Configuration Layer                         │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐   │  │
│  │  │    Config   │ │   Logging   │ │    Environment      │   │  │
│  │  │  Settings   │ │    Setup    │ │     Variables       │   │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘   │  │
│  └────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────-┘
```

### 3. Agent Network Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                       Docker Network: agents-net                │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                 Master Orchestrator                       │  │
│  │               (orchestrator/orchestrator.py)              │  │
│  │  ┌─────────────┐ ┌─────────────--┐ ┌─────────────────────┐│  │
│  │  │   Network   │ │ Infrastructure| │   Agent Builder     ││  │
│  │  │   Setup     │ │   Manager     │ │  & Deployment       ││  │
│  │  └─────────────┘ └─────────────--┘ └─────────────────────┘│  │
│  │  ┌─────────────┐ ┌─────────────-┐ ┌─────────────────────┐ │  │
│  │  │   Health    │ │ Observability│ │    Registry         │ │  │
│  │  │   Monitor   │ │  Injection   │ │   Management        │ │  │
│  │  └─────────────┘ └─────────────-┘ └─────────────────────┘ │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                │                                │
│    ┌───────────────────────────┼───────────────────────────┐    │
│    │                           │                           │    │
│    ▼                           ▼                           ▼    │
│  ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐    │
│  │ Document Expert │ │ GitHub Agent    │ │  Translator     │    │
│  │                 │ │                 │ │                 │    │
│  │ FastAPI Server  │ │ FastAPI Server  │ │ FastAPI Server  │    │
│  │ LangChain Core  │ │ GitHub API      │ │ Translation API │    │
│  │ PDF Processing  │ │ Code Analysis   │ │ Multi-Language  │    │
│  │ Port: 8001      │ │ Port: 8002      │ │ Port: 8003      │    │
│  │ + LangTrace     │ │ + LangTrace     │ │ + LangTrace     │    │
│  └─────────────────┘ └─────────────────┘ └─────────────────┘    │
│                                                                 │
│  ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐    │
│  │Compliance Check │ │   Future Agent  │ │   Future Agent  │    │
│  │                 │ │                 │ │                 │    │
│  │ Regulatory Rules│ │     TBD         │ │      TBD        │    │
│  │ Validation Logic│ │                 │ │                 │    │
│  │ Port: 8004      │ │ Port: 8005      │ │ Port: 8006      │    │
│  │ + LangTrace     │ │ + LangTrace     │ │ + LangTrace     │    │
│  └─────────────────┘ └─────────────────┘ └─────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

### 4. Orchestrator Architecture

The Nasiko Orchestrator (`orchestrator/orchestrator.py`) is the central automation system that handles the complete deployment and management of the entire platform:

```
┌─────────────────────────────────────────────────────────────────┐
│                        Orchestrator Flow                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  1. Network Preparation                                         │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │ • Create docker networks (agents-net, app-network)        │  │
│  │ • Scan agent requirements for external networks           │  │
│  │ • Create all required external networks                   │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                │                                │
│                                ▼                                │
│  2. Infrastructure Deployment                                   │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │ • Start LangTrace (observability dashboard)               │  │
│  │ • Wait for LangTrace to be ready                          │  │
│  │ • Start Nasiko backend API                                │  │
│  │ • Wait for API to be ready                                │  │
│  │ • Start Nasiko web frontend                               │  │
│  │ • Wait for web to be ready                                │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                │                                │
│                                ▼                                │
│  3. Agent Processing & Deployment                               │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │ For each agent in agents/ directory:                      │  │
│  │                                                           │  │
│  │ ┌───────────────────────────────────────────────────────┐ │  │
│  │ │ A. Validation Phase                                   │ │  │
│  │ │ • Check docker-compose.yml exists                     │ │  │
│  │ │ • Validate container names match folder names         │ │  │
│  │ │ • Load capabilities.json if available                 │ │  │
│  │ └───────────────────────────────────────────────────────┘ │  │
│  │                                                           │  │
│  │ ┌───────────────────────────────────────────────────────┐ │  │
│  │ │ B. Instrumentation Phase                              │ │  │
│  │ │ • Copy agent to temporary directory                   │ │  │
│  │ │ • Inject LangTrace configuration (langtrace_config.py)│ │  │
│  │ │ • Modify main.py to import observability              │ │  │
│  │ │ • Update Dockerfile with telemetry dependencies       │ │  │
│  │ │ • Set environment variables (OTEL, LangTrace)         │ │  │
│  │ └───────────────────────────────────────────────────────┘ │  │
│  │                                                           │  │
│  │ ┌───────────────────────────────────────────────────────┐ │  │
│  │ │ C. Build & Deploy Phase                               │ │  │
│  │ │ • Build instrumented Docker image                     │ │  │
│  │ │ • Update docker-compose.yml with:                     │ │  │
│  │ │   - agents-net network attachment                     │ │  │
│  │ │   - instrumented image reference                      │ │  │
│  │ │ • Deploy agent container                              │ │  │
│  │ │ • Wait for container health                           │ │  │
│  │ └───────────────────────────────────────────────────────┘ │  │
│  │                                                           │  │
│  │ ┌───────────────────────────────────────────────────────┐ │  │
│  │ │ D. Registration Phase                                 │ │  │
│  │ │ • Get agent's runtime port mapping                    │ │  │
│  │ │ • Build registry data from capabilities.json          │ │  │
│  │ │ • Register agent with Nasiko API                      │ │  │
│  │ │ • Update agent registry with current URL              │ │  │
│  │ └───────────────────────────────────────────────────────┘ │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                │                                │
│                                ▼                                │
│  4. System Ready                                                │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │ • All services running and healthy                        │  │
│  │ • All agents deployed with observability                  │  │
│  │ • Agent registry populated and up-to-date                 │  │
│  │ • Complete observability stack operational                │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

#### Orchestrator Features

**Automated Network Management:**
- Creates Docker networks (`agents-net`, `app-network`)
- Scans agent docker-compose files for external network requirements
- Creates external networks automatically

**Infrastructure Orchestration:**
- Deploys LangTrace observability dashboard
- Starts MongoDB and Redis databases
- Deploys main Nasiko API backend
- Starts Flutter web frontend

**Agent Instrumentation:**
- Injects comprehensive observability into each agent
- Adds LangTrace, OpenTelemetry instrumentation
- Supports multiple AI libraries (OpenAI, Anthropic, Google, etc.)
- Adds database, web framework, and HTTP client instrumentation

**Redis Stream Processing:**
- Listens for orchestration commands via Redis streams
- Handles asynchronous agent deployment requests
- Processes upload commands from the backend API
- Updates deployment status back to the database

**Registry Management:**
- Automatically registers agents with the platform
- Updates agent URLs with runtime port mappings
- Handles agent metadata from capabilities.json

**Health Monitoring:**
- Waits for each component to be ready before proceeding
- Validates container states and health
- Provides comprehensive error reporting

### 4. Observability Stack

```
┌─────────────────────────────────────────────────────────────────┐
│                    Observability Architecture                   │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                    LangTrace Dashboard                    │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐  │  │
│  │  │    Traces   │ │   Metrics   │ │       Costs         │  │  │
│  │  │   Viewer    │ │  Dashboard  │ │    Monitoring       │  │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘  │  │
│  │                     Port: 3000                            │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                │                                │
│                                ▼                                │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                  OpenTelemetry Layer                      │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐  │  │
│  │  │    OTEL     │ │ Collector   │ │   Instrumentation   │  │  │
│  │  │  Exporter   │ │  (4318)     │ │     Libraries       │  │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘  │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                │                                │
│                                ▼                                │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                    Data Collection                        │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐  │  │
│  │  │    Traces   │ │    Logs     │ │       Metrics       │  │  │
│  │  │ (Requests)  │ │  (Errors)   │ │   (Performance)     │  │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘  │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                │                                │
│                                ▼                                │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                   Data Sources                            │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐  │  │
│  │  │   FastAPI   │ │   Agents    │ │     Database        │  │  │
│  │  │Application  │ │ (All Ports) │ │    Operations       │  │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘  │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## Data Flow Architecture

### 1. Agent Registration Flow

```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│   Client    │    │   FastAPI   │    │   Service   │    │  Database   │
│ (Frontend)  │    │    API      │    │   Layer     │    │ (MongoDB)   │
└──────┬──────┘    └──────┬──────┘    └──────┬──────┘    └──────┬──────┘
       │                  │                  │                  │
       │ POST /registries │                  │                  │
       ├─────────────────►│                  │                  │
       │                  │                  │                  │
       │                  │ validate_data    │                  │
       │                  ├─────────────────►│                  │
       │                  │                  │                  │
       │                  │                  │ create_registry  │
       │                  │                  ├─────────────────►│
       │                  │                  │                  │
       │                  │                  │    registry_id   │
       │                  │                  │◄─────────────────┤
       │                  │                  │                  │
       │                  │   registry_obj   │                  │
       │                  │◄─────────────────┤                  │
       │                  │                  │                  │
       │    response      │                  │                  │
       │◄─────────────────┤                  │                  │
       │                  │                  │                  │
       │                  │ notify_agents    │                  │
       │                  ├─────────────────►│                  │
       │                  │                  │                  │
       │                  │ update_traces    │                  │
       │                  ├─────────────────►│                  │
       │                  │                  │                  │
```

### 2. Agent Communication Flow (Direct via Kong)

```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│   Client    │    │Kong Gateway │    │   Agent     │
│             │    │   (9100)    │    │ Container   │
└──────┬──────┘    └──────┬──────┘    └──────┬──────┘
       │                  │                  │
       │ Chat Request     │                  │
       │ /document-expert/chat               │
       ├─────────────────►│                  │
       │                  │                  │
       │                  │ Route to Agent   │
       │                  ├─────────────────►│
       │                  │                  │
       │                  │                  │ Process Request
       │                  │                  ├──────────────┐
       │                  │                  │              │
       │                  │                  │◄─────────────┘
       │                  │                  │
       │                  │   Response       │
       │                  │◄─────────────────┤
       │                  │                  │
       │    Response      │                  │
       │◄─────────────────┤                  │
       │                  │                  │

Note: Chat requests bypass Nasiko Backend (8000) completely.
Backend only handles agent registration, upload, and management.
Kong Gateway provides direct routing to agents for all chat operations.
```

### 3. Upload Agent Flow (with Redis Stream Processing)

```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│   Client    │    │ Nasiko API  │    │    Redis    │    │Orchestrator │
│ (CLI/Web)   │    │  Backend    │    │   Streams   │    │   Stream    │
│             │    │   (8000)    │    │   (6379)    │    │  Listener   │
└──────┬──────┘    └──────┬──────┘    └──────┬──────┘    └──────┬──────┘
       │                  │                  │                  │
       │ Upload Agent     │                  │                  │
       ├─────────────────►│                  │                  │
       │                  │                  │                  │
       │                  │ Store Upload     │                  │
       │                  ├─────────────────► │                  │
       │                  │ Status           │                  │
       │                  │                  │                  │
       │                  │ Send Command     │                  │
       │                  │ to Stream        │                  │
       │                  ├─────────────────►│                  │
       │                  │                  │                  │
       │                  │                  │ Read Stream      │
       │                  │                  ├─────────────────►│
       │                  │                  │ Message          │
       │                  │                  │                  │
       │                  │                  │                  │ Build & Deploy
       │                  │                  │                  ├───────────────┐
       │                  │                  │                  │              │
       │                  │                  │                  │◄──────────────┘
       │                  │                  │                  │
       │                  │ Update Status    │                  │
       │                  │◄─────────────────┼──────────────────┤
       │                  │                  │                  │
       │ Status Updates   │                  │                  │
       │◄─────────────────┤                  │                  │
       │                  │                  │                  │

Note: The Redis Stream Listener MUST be running as a separate process
for the upload agent flow to work. It processes orchestration commands
asynchronously and handles Docker image building and deployment.
```

### 4. Observability Data Flow

```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│   Agents    │    │ OTEL        │    │ LangTrace   │    │  Dashboard  │
│ (All Types) │    │ Collector   │    │   API       │    │   (Web UI)  │
└──────┬──────┘    └──────┬──────┘    └──────┬──────┘    └──────┬──────┘
       │                  │                  │                  │
       │ Trace Data       │                  │                  │
       ├─────────────────►│                  │                  │
       │                  │                  │                  │
       │                  │ Process & Format │                  │
       │                  ├────────────────┐ │                  │
       │                  │                │ │                  │
       │                  │◄───────────────┘ │                  │
       │                  │                  │                  │
       │                  │ Send Traces      │                  │
       │                  ├─────────────────►│                  │
       │                  │                  │                  │
       │                  │                  │ Store Traces     │
       │                  │                  ├────────────────┐ │
       │                  │                  │                │ │
       │                  │                  │◄───────────────┘ │
       │                  │                  │                  │
       │                  │                  │                  │ View Traces
       │                  │                  │                  ├────────────►
       │                  │                  │ Query Traces     │
       │                  │                  │◄─────────────────┤
       │                  │                  │                  │
       │                  │                  │ Trace Data       │
       │                  │                  ├─────────────────►│
       │                  │                  │                  │
```

## Deployment Architecture

### Development Environment

```
┌─────────────────────────────────────────────────────────────────┐
│                    Development Environment                      │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  Host Machine (localhost)                                       │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                                                           │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐  │  │
│  │  │   Python    │ │   Flutter   │ │      Docker         │  │  │
│  │  │ Environment │ │    Web      │ │    Containers       │  │  │
│  │  │ (FastAPI)   │ │  Frontend   │ │   (Agents + DBs)    │  │  │
│  │  │ Port: 8000  │ │ Port: 4000  │ │  Ports: 8001-8006   │  │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘  │  │
│  │                                                           │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐  │  │
│  │  │   MongoDB   │ │  LangTrace  │ │   File System       │  │  │
│  │  │ Container   │ │  Container  │ │    (Local)          │  │  │
│  │  │Port: 27017  │ │ Port: 3000  │ │                     │  │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘  │  │
│  │                                                           │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

### Production Environment

```
┌─────────────────────────────────────────────────────────────────┐
│                    Production Environment                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  Cloud Infrastructure (AWS/GCP/Azure)                           │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                                                           │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐  │  │
│  │  │    CDN      │ │Load Balancer│ │      WAF            │  │  │
│  │  │ (Static)    │ │(Application)│ │   (Security)        │  │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘  │  │
│  │                                                           │  │
│  │  Kubernetes Cluster                                       │  │
│  │  ┌───────────────────────────────────────────────────────┐│  │
│  │  │                                                       ││  │
│  │  │ ┌─────────────┐ ┌─────────────┐ ┌────────────────────┐││  │
│  │  │ │  Frontend   │ │   Backend   │ │      Agents        │││  │
│  │  │ │    Pods     │ │    Pods     │ │      Pods          │││  │
│  │  │ │ (3 replicas)│ │(5 replicas) │ │   (2-3 each)       │││  │
│  │  │ └─────────────┘ └─────────────┘ └────────────────────┘││  │
│  │  │                                                       ││  │
│  │  │ ┌─────────────┐ ┌─────────────┐ ┌────────────────────┐││  │
│  │  │ │  MongoDB    │ │   Redis     │ │   Observability    │││  │
│  │  │ │ StatefulSet │ │ Deployment  │ │      Stack         │││  │
│  │  │ │(Replica Set)│ │  (Cache)    │ │  (Monitoring)      │││  │
│  │  │ └─────────────┘ └─────────────┘ └────────────────────┘││  │
│  │  │                                                       ││  │
│  │  └───────────────────────────────────────────────────────┘│  │
│  │                                                           │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌────────────────────┐   │  │
│  │  │   Storage   │ │   Secrets   │ │    Networking      │   │  │
│  │  │  (EBS/PD)   │ │ Management  │ │    (VPC/CNI)       │   │  │
│  │  └─────────────┘ └─────────────┘ └────────────────────┘   │  │
│  │                                                           │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## Security Architecture

### Authentication & Authorization

```
┌─────────────────────────────────────────────────────────────────┐
│                      Security Layer                             │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                   API Gateway                             │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐  │  │
│  │  │Rate Limiting│ │   WAF       │ │   TLS Termination   │  │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘  │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                │                                │
│                                ▼                                │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                Authentication Layer                       │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐  │  │
│  │  │    JWT      │ │    OAuth    │ │       RBAC          │  │  │
│  │  │   Tokens    │ │   Provider  │ │   (Role Based)      │  │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘  │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                │                                │
│                                ▼                                │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                  Authorization Layer                      │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────────────┐  │  │
│  │  │ Permissions │ │   Policies  │ │    Agent Access     │  │  │
│  │  │   Matrix    │ │  (ABAC)     │ │     Controls        │  │  │
│  │  └─────────────┘ └─────────────┘ └─────────────────────┘  │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                │                                │
│                                ▼                                │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                   Security Controls                       │  │
│  │  ┌─────────────┐ ┌─────────────----┐ ┌─────────────────┐  │  │
│  │  │   Audit     │ │ Encryption      │ │    Secrets      │  │  │
│  │  │   Logging   │ │(At Rest/Transit)│ │   Management    │  │  │
│  │  └─────────────┘ └────────────----─┘ └─────────────────┘  │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## Scalability Considerations

### Horizontal Scaling Pattern

1. **Stateless Services**: All services are designed to be stateless
2. **Load Distribution**: Use load balancers at each layer
3. **Database Scaling**: MongoDB replica sets and sharding
4. **Caching Strategy**: Redis for frequently accessed data
5. **Agent Scaling**: Dynamic agent provisioning based on demand

### Performance Optimization

1. **Connection Pooling**: Database connection management
2. **Async Operations**: FastAPI async/await throughout
3. **Caching Layers**: Multi-level caching strategy
4. **Resource Limits**: Container resource constraints
5. **Monitoring**: Comprehensive performance monitoring

## Technology Stack Summary

### Backend Technologies
- **FastAPI**: Web framework with async support
- **Pydantic**: Data validation and serialization
- **Motor**: Async MongoDB driver
- **Uvicorn**: ASGI server implementation

### Database & Storage
- **MongoDB**: Primary database for all entities
- **Redis**: Caching and session storage
- **File System**: Local/S3 for document storage

### AI & ML Stack
- **LangChain**: Agent orchestration framework
- **OpenAI**: GPT models for language processing
- **Anthropic**: Claude models as alternative
- **Google**: Gemini for multi-modal capabilities

### Infrastructure
- **Docker**: Containerization platform
- **Kubernetes**: Container orchestration (production)
- **Nginx**: Reverse proxy and load balancer
- **Traefik**: Service mesh (alternative)

### Observability
- **OpenTelemetry**: Distributed tracing standard
- **LangTrace**: AI-specific observability platform
- **Prometheus**: Metrics collection
- **Grafana**: Metrics visualization

## Future Architecture Enhancements

### Planned Improvements
1. **Event-Driven Architecture**: Implement message queues for agent communication
2. **Service Mesh**: Add Istio or Linkerd for service-to-service communication
3. **Multi-Region**: Support for multi-region deployments
4. **Edge Computing**: Edge deployment for low-latency requirements
5. **ML Pipeline**: Integrated ML model training and deployment pipeline

### Integration Roadmap
1. **External APIs**: Support for more third-party integrations
2. **Webhook System**: Real-time notifications and callbacks
3. **Plugin Architecture**: Extensible plugin system for custom agents
4. **Marketplace**: Agent marketplace for community contributions