# Open Source Separation Architecture

## High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     NASIKO ENTERPRISE                           │
│                   (Private Repository)                          │
│                                                                 │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                                                            │ │
│  │              NASIKO CORE (Git Subtree)                     │ │
│  │                (Public Repository)                         │ │
│  │                                                            │ │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌──────────┐       │ │
│  │  │ Backend │  │Frontend │  │   CLI   │  │Orchestra-│       │ │
│  │  │  (OSS)  │  │  (OSS)  │  │  (OSS)  │  │ tor(OSS) │       │ │
│  │  └────┬────┘  └────┬────┘  └────┬────┘  └─────┬────┘       │ │
│  │       │            │            │             │            │ │
│  │       │   Plugin Interfaces     │             │            │ │
│  │       ▼            ▼            ▼             ▼            │ │
│  │  ┌────────────────────────────────────────────────────┐    │ │
│  │  │         Plugin Loader (NASIKO_EDITION)             │    │ │
│  │  └───────────────────┬────────────────────────────────┘    │ │
│  │                      │                                     │ │
│  └──────────────────────┼─────────────────────────────────────┘ │
│                         │                                       │
│         ┌───────────────┴────────────────┐                      │
│         │                                │                      │
│    Community Mode                  Enterprise Mode              │
│         │                                │                      │
│    ┌────▼────┐                      ┌───▼──────────────────┐    │
│    │ No Auth │                      │  Enterprise Plugins  │    │
│    │ Basic   │                      │                       │   │
│    │ Router  │                      │  ┌────────────────┐  │  │
│    └─────────┘                      │  │ Auth Service   │  │  │
│                                     │  │ (JWT + ACL)    │  │  │
│                                     │  └────────────────┘  │  │
│                                     │  ┌────────────────┐  │  │
│                                     │  │ AI Router      │  │  │
│                                     │  │ (FAISS + LLM)  │  │  │
│                                     │  └────────────────┘  │  │
│                                     │  ┌────────────────┐  │  │
│                                     │  │ CLI Extensions │  │  │
│                                     │  │ (User/Access)  │  │  │
│                                     │  └────────────────┘  │  │
│                                     │  ┌────────────────┐  │  │
│                                     │  │ Web Extensions │  │  │
│                                     │  │ (ACL UI)       │  │  │
│                                     │  └────────────────┘  │  │
│                                     └──────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Plugin System Architecture

```
┌────────────────────────────────────────────────────────────────┐
│                    APPLICATION STARTUP                         │
└──────────────────────────┬─────────────────────────────────────┘
                           │
                           ▼
                  ┌────────────────┐
                  │ Load .env file │
                  │ NASIKO_EDITION │
                  └────────┬───────┘
                           │
                           ▼
                  ┌─────────────────┐
                  │ Plugin Loader   │
                  │ Initialization  │
                  └────────┬────────┘
                           │
             ┌─────────────┴─────────────┐
             │                           │
        Community                    Enterprise
             │                           │
             ▼                           ▼
    ┌────────────────┐         ┌────────────────────┐
    │  Load Default  │         │ Load Enterprise    │
    │  Plugins:      │         │ Plugins:           │
    │                │         │                    │
    │  • NoAuthPlugin│         │ • AuthPlugin       │
    │  • BasicRouter │         │   (JWT + ACL)      │
    │                │         │ • AIRouter         │
    │                │         │   (FAISS + LLM)    │
    └────────┬───────┘         └──────────┬─────────┘
             │                            │
             └────────────┬───────────────┘
                          │
                          ▼
               ┌──────────────────────┐
               │  Register Plugins    │
               │  in App Context      │
               └──────────┬───────────┘
                          │
                          ▼
               ┌──────────────────────┐
               │  Load Routes         │
               │  (Conditional)       │
               └──────────┬───────────┘
                          │
                          ▼
               ┌──────────────────────┐
               │  Start Services      │
               └──────────────────────┘
```

---

## Backend Request Flow

### Community Edition Flow

```
┌──────┐
│Client│
└───┬──┘
    │
    │ POST /api/v1/agents
    ▼
┌─────────────────┐
│  Backend API    │
│  (No Auth)      │
└────────┬────────┘
         │
         │ plugin_loader.auth_plugin = NoAuthPlugin()
         ▼
    ┌────────────┐
    │ NoAuthPlugin│──────► Returns None (no authentication)
    └────────────┘
         │
         ▼
┌──────────────────┐
│  Service Layer   │  Processes request without user context
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│   Repository     │  Stores agent in MongoDB
└────────┬─────────┘
         │
         ▼
    ┌────────┐
    │Response│ {"id": "agent-123", "status": "created"}
    └────────┘
```

### Enterprise Edition Flow

```
┌──────┐
│Client│
└───┬──┘
    │
    │ POST /api/v1/agents
    │ Authorization: Bearer <JWT>
    ▼
┌─────────────────────┐
│   Backend API       │
│ (Auth Middleware)   │
└──────────┬──────────┘
           │
           │ plugin_loader.auth_plugin = EnterpriseAuthPlugin()
           ▼
    ┌─────────────────┐
    │EnterpriseAuth   │
    │Plugin           │
    └────────┬────────┘
             │
             │ Validate JWT
             ▼
    ┌─────────────────┐
    │  Auth Service   │──► Validates token, checks Redis
    │  (Microservice) │    Returns user_id, permissions
    └────────┬────────┘
             │
             │ {"user_id": "user-123", "is_super_user": false}
             ▼
┌──────────────────────┐
│   Service Layer      │  Processes with user context
│                      │  owner_id = user-123
└────────┬─────────────┘
         │
         ▼
┌──────────────────────┐
│   Repository         │  Stores agent + owner_id
└────────┬─────────────┘
         │
         ▼
┌──────────────────────┐
│  Auth Client         │  Create agent permissions
│                      │  auth_service.create_permissions(agent_id, owner_id)
└────────┬─────────────┘
         │
         ▼
    ┌────────┐
    │Response│ {"id": "agent-123", "owner": "user-123"}
    └────────┘
```

---

## Router Service Architecture

### Community Edition - Basic Router

```
┌──────────┐
│User Query│ "Find a document about Python"
└────┬─────┘
     │
     ▼
┌────────────────────┐
│ BasicRouter Plugin │
└─────────┬──────────┘
          │
          │ 1. Get all agents from registry
          ▼
     ┌──────────┐
     │ MongoDB  │ → [agent1, agent2, agent3, ...]
     └────┬─────┘
          │
          │ 2. Keyword matching
          ▼
┌──────────────────────┐
│ Scoring Algorithm:   │
│ • Check query in     │
│   agent name (+10)   │
│ • Check query in     │
│   description (+5)   │
│ • Check query in     │
│   tags (+3)          │
└──────────┬───────────┘
           │
           │ 3. Sort by score
           ▼
      ┌─────────┐
      │ Top     │ agent with highest score
      │ Match   │
      └────┬────┘
           │
           ▼
     ┌──────────┐
     │ Response │ {"agent": "document-expert", "url": "..."}
     └──────────┘
```

### Enterprise Edition - AI Router

```
┌──────────┐
│User Query│ "Find a document about Python"
└────┬─────┘
     │
     ▼
┌──────────────────────────┐
│ EnterpriseRouter Plugin  │
└──────────┬───────────────┘
           │
           │ 1. Semantic search
           ▼
    ┌──────────────┐
    │ FAISS Vector │ Embedding of query
    │ Store        │
    └──────┬───────┘
           │
           │ similarity_search(query, k=2)
           ▼
    ┌──────────────┐
    │ Top 2 Agents │ [agent1, agent2]
    └──────┬───────┘
           │
           │ 2. LLM Selection
           ▼
    ┌──────────────────┐
    │ OpenRouter LLM   │
    │ (Gemini 2.5)     │
    └──────┬───────────┘
           │
           │ Prompt: "Given user query and 2 agents, select best one"
           ▼
    ┌──────────────┐
    │ LLM analyzes │ Reasoning: "Agent 1 specializes in document
    │ capabilities │ processing and mentions Python support"
    └──────┬───────┘
           │
           │ 3. Final decision
           ▼
      ┌─────────┐
      │ Best    │ agent with AI confidence score
      │ Agent   │
      └────┬────┘
           │
           ▼
     ┌──────────┐
     │ Response │ {"agent": "document-expert", "confidence": 0.95}
     └──────────┘
```

---

## Data Flow: Agent Deployment

### Community Edition

```
1. User uploads agent
   ├─► Backend receives upload
   ├─► Stores in registry (no owner_id)
   ├─► Triggers K8s build job
   └─► Deploys to K8s

2. Agent accessible to all (no ACL)
   └─► Kong routes traffic directly
```

### Enterprise Edition

```
1. User uploads agent (with JWT token)
   ├─► Auth middleware validates token
   │   └─► Extracts user_id from token
   │
   ├─► Backend stores agent
   │   └─► Sets owner_id = user_id
   │
   ├─► Creates agent permissions
   │   ├─► MongoDB: {agent_id, owner_id, can_access: []}
   │   └─► Redis: Sets for fast lookup
   │       ├─► agent:{id}:users → Set(owner_id)
   │       └─► user:{owner_id}:agents → Set(agent_id)
   │
   ├─► Triggers K8s build job
   └─► Deploys to K8s

2. Access control enforced
   ├─► User requests agent
   ├─► Auth middleware checks:
   │   ├─► Is user super_user? → Allow
   │   ├─► Is user owner? → Allow
   │   └─► Is user in can_access list? → Allow/Deny
   │
   └─► Kong routes if authorized
```

---

## Directory Structure Comparison

### Community Edition (nasiko-core)

```
nasiko-core/
├── app/                          # Backend API
│   ├── api/
│   │   ├── routes/
│   │   │   ├── registry_routes.py
│   │   │   ├── agent_upload_routes.py
│   │   │   ├── router_routes.py        # NEW: Basic routing
│   │   │   └── ...
│   │   └── handlers/
│   ├── service/
│   ├── repository/
│   ├── entity/
│   ├── pkg/
│   │   ├── config/
│   │   └── plugins/                     # NEW: Plugin system
│   │       ├── loader.py
│   │       ├── auth_plugin.py
│   │       └── router_plugin.py
│   └── main.py
│
├── web/                          # Frontend
│   ├── lib/
│   │   ├── presentation/
│   │   │   ├── agents/
│   │   │   ├── query/
│   │   │   └── ...                      # No access_control/
│   │   ├── data/
│   │   └── core/
│   │       └── config.dart              # NEW: Edition config
│   └── pubspec.yaml
│
├── cli/                          # CLI
│   ├── groups/
│   │   ├── agent_group.py
│   │   └── ...                          # No access_group.py
│   ├── commands/
│   └── main.py
│
├── orchestrator/                 # Deployment orchestration
│   ├── orchestrator.py
│   ├── agent_builder.py
│   └── ...
│
├── agent-gateway/                # Kong + Service Discovery
│   ├── registry/
│   │   └── registry.py
│   └── docker-compose.yml
│
├── agents/                       # Sample agents
│   ├── document-expert/
│   └── ...
│
├── k8s/                          # Kubernetes manifests
├── terraform/                    # Infrastructure as Code
├── docs/                         # Documentation
└── pyproject.toml                # Python dependencies (minimal)
```

### Enterprise Edition (nasiko-enterprise)

```
nasiko-enterprise/
├── nasiko-core/                  # Git subtree → Public repo
│   └── (Same structure as above)
│
├── enterprise/                   # Proprietary features
│   │
│   ├── auth-service/             # Authentication & ACL microservice
│   │   ├── main.py
│   │   ├── service/
│   │   │   └── auth_service.py
│   │   ├── repository/
│   │   ├── entity/
│   │   ├── api/
│   │   ├── Dockerfile
│   │   └── requirements.txt
│   │
│   ├── router/                   # AI-powered routing service
│   │   ├── src/
│   │   │   ├── main.py
│   │   │   ├── core/
│   │   │   │   ├── routing_engine.py
│   │   │   │   └── vector_store.py
│   │   │   └── services/
│   │   ├── Dockerfile
│   │   └── requirements.txt
│   │
│   ├── backend-plugins/          # Backend integrations
│   │   └── auth/
│   │       ├── plugin.py         # AuthPlugin implementation
│   │       ├── middleware.py     # Auth middleware
│   │       ├── client.py         # Auth service client
│   │       └── routes.py         # Superuser routes
│   │
│   ├── router-plugin/            # Router integration
│   │   └── plugin.py             # RouterPlugin implementation
│   │
│   ├── cli-extensions/           # CLI additions
│   │   ├── groups/
│   │   │   └── access_group.py
│   │   └── commands/
│   │       ├── access.py
│   │       └── user_management.py
│   │
│   ├── web-extensions/           # Frontend additions
│   │   └── lib/
│   │       ├── presentation/
│   │       │   └── access_control/
│   │       └── data/
│   │           ├── repositories/
│   │           ├── datasources/
│   │           └── models/
│   │
│   ├── kong-plugins/             # Kong plugins
│   │   └── chat-logger/
│   │
│   └── scripts/                  # Migration & utilities
│       └── migrate_from_community.py
│
├── docker-compose.enterprise.yml # Includes nasiko-core + enterprise services
├── .env.enterprise.example
└── pyproject.toml                # Includes enterprise dependencies
```

---

## Service Communication

### Community Edition Services

```
┌────────────┐
│   Client   │
└─────┬──────┘
      │
      ▼
┌────────────┐     ┌──────────┐     ┌─────────┐
│  Backend   │────►│ MongoDB  │     │  Redis  │
│  (Port     │     │          │     │         │
│   8000)    │◄────┤          │────►│         │
└─────┬──────┘     └──────────┘     └─────────┘
      │
      │ Deploys agents
      ▼
┌────────────┐     ┌──────────┐
│ Kubernetes │────►│ BuildKit │
│   Cluster  │     │          │
└─────┬──────┘     └──────────┘
      │
      │ Agents deployed as K8s services
      ▼
┌────────────┐
│    Kong    │ Auto-discovers agents
│  Gateway   │
│ (Port 9100)│
└────────────┘
```

### Enterprise Edition Services

```
┌────────────┐
│   Client   │
└─────┬──────┘
      │
      │ JWT Token
      ▼
┌────────────┐     ┌──────────────┐
│  Backend   │────►│ Auth Service │
│  (Port     │     │  (Port 8001) │
│   8000)    │◄────┤              │
└─────┬──────┘     └──────┬───────┘
      │                   │
      │                   ▼
      │            ┌──────────┐     ┌─────────┐
      │            │ MongoDB  │     │  Redis  │
      │            │          │◄───►│ (Perms) │
      │            └──────────┘     └─────────┘
      │
      │ Route queries
      ▼
┌────────────┐     ┌──────────┐
│    AI      │────►│  FAISS   │
│  Router    │     │ Vector DB│
│ (Port 8005)│     └──────────┘
└────────────┘
      │
      │ Select best agent
      ▼
┌────────────┐     ┌──────────┐
│    Kong    │────►│   Chat   │
│  Gateway   │     │  Logger  │
│ (Port 9100)│     │  Plugin  │
└─────┬──────┘     └──────────┘
      │
      │ Forward to agent
      ▼
┌────────────┐
│   Agent    │
│   (K8s)    │
└────────────┘
```

---

## Authentication Flow (Enterprise Only)

```
┌─────────┐
│  User   │
└────┬────┘
     │
     │ 1. Login request
     │ POST /auth/users/authenticate
     │ Body: {access_key, access_secret}
     ▼
┌──────────────┐
│Auth Service  │
└──────┬───────┘
       │
       │ 2. Validate credentials
       ▼
  ┌─────────┐
  │ MongoDB │ Check users collection
  └────┬────┘ bcrypt.checkpw(password, stored_hash)
       │
       │ 3. Generate JWT
       ▼
  ┌─────────────────┐
  │ JWT Token       │ Claims: {user_id, is_super_user, exp}
  │ HS256 signed    │
  │ 12h expiration  │
  └────┬────────────┘
       │
       │ 4. Store in Redis
       ▼
  ┌─────────┐
  │  Redis  │ SET token:{token_id} → {user_id, metadata}
  └────┬────┘ EXPIRE 12 hours
       │
       ▼
  ┌─────────┐
  │Response │ {token: "eyJ...", expires_in: 43200}
  └─────────┘

--- Subsequent Requests ---

┌─────────┐
│  User   │
└────┬────┘
     │
     │ API request with Authorization: Bearer <token>
     ▼
┌──────────────┐
│  Backend     │
│  Middleware  │
└──────┬───────┘
       │
       │ POST /auth/validate
       ▼
┌──────────────┐
│Auth Service  │
└──────┬───────┘
       │
       │ 1. Check Redis
       ▼
  ┌─────────┐
  │  Redis  │ GET token:{token_id}
  └────┬────┘
       │
       │ Found? → Valid ✅
       │ Not found? → Check MongoDB
       ▼
  ┌─────────┐
  │ MongoDB │ Check if user exists & active
  └────┬────┘
       │
       ▼
  ┌─────────┐
  │Response │ {valid: true, user_id, is_super_user}
  └─────────┘
```

---

## Permission Check Flow (Enterprise Only)

```
User requests access to Agent X
         │
         ▼
┌──────────────────┐
│ Auth Middleware  │ Extracts user_id from JWT
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│ Permission Check │
└────────┬─────────┘
         │
         ▼
    Is super_user?
         │
    ┌────┴────┐
    │         │
   Yes       No
    │         │
    │         ▼
    │    Is owner?
    │         │
    │    ┌────┴────┐
    │   Yes       No
    │    │         │
    │    │         ▼
    │    │    Check Redis set
    │    │         │
    │    │    SISMEMBER agent:{agent_id}:users {user_id}
    │    │         │
    │    │    ┌────┴────┐
    │    │   Yes       No
    │    │    │         │
    ▼    ▼    ▼         ▼
  Allow  Allow  Allow  Deny
    │    │    │         │
    └────┴────┴─────────┘
         │
         ▼
   Process Request
```

---

## Deployment Flow Comparison

### Community: Agent Upload

```
User → CLI/Web
       │
       └─► POST /api/v1/upload-agents/github
           Body: {git_url, name}
           │
           ▼
       ┌────────────────┐
       │  Backend API   │ No auth check
       └────────┬───────┘
                │
                │ 1. Store in registry (no owner)
                ▼
           ┌─────────┐
           │ MongoDB │ Insert {id, name, git_url, status: "pending"}
           └────┬────┘
                │
                │ 2. Send to Redis stream
                ▼
           ┌─────────┐
           │  Redis  │ XADD upload_stream {git_url, agent_id}
           └────┬────┘
                │
                │ 3. Redis listener picks up
                ▼
        ┌─────────────────┐
        │ Redis Stream    │
        │ Listener        │
        └────────┬────────┘
                 │
                 │ 4. Build image
                 ▼
            ┌──────────┐
            │ K8s Job  │ BuildKit builds image
            │ (Build)  │
            └────┬─────┘
                 │
                 │ 5. Deploy
                 ▼
            ┌──────────┐
            │ K8s      │ Create Deployment + Service
            │ Deploy   │
            └────┬─────┘
                 │
                 │ 6. Auto-discovery
                 ▼
            ┌──────────┐
            │   Kong   │ Creates route
            └──────────┘
```

### Enterprise: Agent Upload

```
User (with JWT) → CLI/Web
       │
       └─► POST /api/v1/upload-agents/github
           Headers: Authorization: Bearer <JWT>
           Body: {git_url, name}
           │
           ▼
       ┌────────────────┐
       │ Auth Middleware│ Validates JWT, extracts user_id
       └────────┬───────┘
                │
                ▼
       ┌────────────────┐
       │  Backend API   │
       └────────┬───────┘
                │
                │ 1. Store with owner
                ▼
           ┌─────────┐
           │ MongoDB │ Insert {id, name, git_url, owner_id, status}
           └────┬────┘
                │
                │ 2. Create permissions
                ▼
       ┌────────────────┐
       │  Auth Service  │ POST /auth/agents/{id}/permissions
       └────────┬───────┘
                │
                ▼
           ┌─────────┐
           │ MongoDB │ Insert {agent_id, owner_id, can_access: []}
           └────┬────┘
                │
           ┌─────────┐
           │  Redis  │ SADD agent:{id}:users {owner_id}
           │         │ SADD user:{owner_id}:agents {agent_id}
           └────┬────┘
                │
                │ 3. Build & deploy (same as community)
                ▼
           ... (same K8s build/deploy flow) ...
```

---

## Configuration Loading

```
Application Startup
        │
        ▼
    Load .env
        │
        ▼
  NASIKO_EDITION?
        │
   ┌────┴────┐
   │         │
Community  Enterprise
   │         │
   ▼         ▼
┌──────┐  ┌──────────┐
│Vars: │  │Vars:     │
│      │  │          │
│ MONGO│  │ MONGO    │
│ REDIS│  │ REDIS    │
│ ...  │  │ AUTH_URL │
│      │  │ JWT_SEC  │
│      │  │ ROUTER   │
│      │  │ OPENRTR  │
└──┬───┘  └────┬─────┘
   │           │
   └─────┬─────┘
         │
         ▼
   Plugin Loader
         │
         ▼
   Load Plugins
         │
    ┌────┴────┐
    │         │
Community  Enterprise
    │         │
    ▼         ▼
NoAuth    EnterpriseAuth
BasicRtr  AIRouter
    │         │
    └────┬────┘
         │
         ▼
  Register in App
         │
         ▼
  Load Routes
         │
    ┌────┴────┐
    │         │
Community  Enterprise
    │         │
    ▼         ▼
  Core     Core +
  Routes   Auth Routes
    │      + Super Routes
    │         │
    └────┬────┘
         │
         ▼
   Start Services
```

---

This architecture ensures:
1. **Clear separation** between open-source and proprietary code
2. **Plugin-based extensibility** without modifying core
3. **Graceful degradation** when enterprise features unavailable
4. **Single codebase maintenance** via git subtree
5. **No vendor lock-in** for community users
