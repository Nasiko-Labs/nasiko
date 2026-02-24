# Nasiko Open Source Separation Plan

## Overview

This document outlines the strategy for separating Nasiko into:
1. **nasiko-core** (Open Source) - Public repository with core agent registry functionality
2. **nasiko-enterprise** (Proprietary) - Private repository with advanced features (ACL, intelligent router, etc.)

The architecture uses a **plugin-based approach** where the open-source version serves as the foundation, and proprietary features are loaded conditionally based on configuration.

---

## Architecture Strategy

### Repository Structure

```
nasiko-enterprise/                    # Proprietary repository
├── nasiko-core/                      # Git subtree of open-source repo
│   ├── app/                          # Core backend (open source)
│   ├── web/                          # Core frontend (open source)
│   ├── cli/                          # Core CLI (open source)
│   ├── orchestrator/                 # Core orchestrator (open source)
│   └── ...
├── enterprise/                       # Proprietary features
│   ├── auth-service/                 # ACL & user management
│   ├── router/                       # AI-powered intelligent router
│   ├── web-extensions/               # Frontend ACL UI
│   ├── cli-extensions/               # CLI access control commands
│   └── plugins/                      # Additional proprietary plugins
└── docker-compose.enterprise.yml     # Enterprise-specific services
```

### Plugin Loading Mechanism

**Configuration Flag**: `NASIKO_EDITION` environment variable
- `community` - Open source version (default)
- `enterprise` - Proprietary version with all features

---

## Component Separation

### 1. PROPRIETARY COMPONENTS (nasiko-enterprise)

#### 1.1 Auth Service (`enterprise/auth-service/`)
**Rationale**: Complete ACL implementation with user management, agent permissions, JWT tokens

**Files to Move**:
```
/auth-service/                        → enterprise/auth-service/
├── main.py
├── service/auth_service.py           # Core ACL logic
├── repository/auth_repository.py
├── entity/auth_entity.py
├── api/
│   ├── auth_handler.py
│   └── auth_routes.py
├── Dockerfile
├── docker-compose.yml
└── requirements.txt
```

**Integration Points**:
- Backend auth middleware will conditionally load auth service client
- If enterprise: validate tokens with auth service
- If community: use simple token validation or no auth

#### 1.2 Intelligent Router (`enterprise/router/`)
**Rationale**: AI-powered routing with FAISS vector store and LLM selection

**Files to Move**:
```
/agent-gateway/router/                → enterprise/router/
├── src/
│   ├── main.py                       # Router service
│   ├── core/
│   │   ├── routing_engine.py         # Two-stage routing logic
│   │   ├── agent_registry.py         # Agent metadata management
│   │   ├── agent_client.py           # Agent communication
│   │   └── vector_store.py           # FAISS vector store
│   ├── services/
│   │   └── router_orchestrator.py
│   └── entities/
│       └── router_entities.py
├── Dockerfile
├── docker-compose.yml
└── requirements.txt
```

**Replacement for Community Edition**:
Create a basic router in `nasiko-core/app/api/routes/router_routes.py`:
- Simple keyword matching or tag-based routing
- No AI/LLM, no vector search
- Direct agent selection based on capabilities.json tags

#### 1.3 CLI Access Control Extensions (`enterprise/cli-extensions/`)
**Rationale**: User management and agent access control commands

**Files to Move**:
```
/cli/groups/access_group.py           → enterprise/cli-extensions/groups/
/cli/commands/access.py               → enterprise/cli-extensions/commands/
/cli/commands/user_management.py      → enterprise/cli-extensions/commands/
```

**Integration**:
```python
# In nasiko-core/cli/main.py
if os.getenv("NASIKO_EDITION") == "enterprise":
    from enterprise.cli_extensions.groups import access_group, user_group
    app.add_typer(access_group.app, name="access")
    app.add_typer(user_group.app, name="users")
```

#### 1.4 Frontend Access Control UI (`enterprise/web-extensions/`)
**Rationale**: User management screens and agent permission UI

**Files to Move**:
```
/web/lib/presentation/access_control/           → enterprise/web-extensions/presentation/
/web/lib/data/repositories/access_control_repository_impl.dart  → enterprise/web-extensions/data/repositories/
/web/lib/data/datasources/access_control_data_source.dart       → enterprise/web-extensions/data/datasources/
/web/lib/data/models/agent_permissions_model.dart               → enterprise/web-extensions/data/models/
/web/lib/data/models/user_model.dart                            → enterprise/web-extensions/data/models/
```

**Integration**:
```dart
// In nasiko-core/web/lib/main.dart
final bool isEnterprise = const String.fromEnvironment('NASIKO_EDITION', defaultValue: 'community') == 'enterprise';

// Conditionally add routes
if (isEnterprise) {
  routes.add('/access-control': (context) => AccessControlScreen());
  routes.add('/users': (context) => UserManagementScreen());
}
```

#### 1.5 Backend Auth Integration (`enterprise/backend-plugins/auth/`)
**Rationale**: Auth middleware and service client

**Files to Move**:
```
/app/api/auth.py                      → enterprise/backend-plugins/auth/middleware.py
/app/pkg/auth/auth_client.py          → enterprise/backend-plugins/auth/client.py
/app/api/routes/superuser_routes.py   → enterprise/backend-plugins/auth/routes.py
```

**Integration**:
```python
# In nasiko-core/app/main.py
if os.getenv("NASIKO_EDITION") == "enterprise":
    from enterprise.backend_plugins.auth.middleware import get_current_user, get_super_user
    from enterprise.backend_plugins.auth.routes import router as auth_router
    app.include_router(auth_router)
else:
    # Simple auth or no auth
    async def get_current_user():
        return None  # No authentication in community edition
```

#### 1.6 Kong Chat Logger Plugin (`enterprise/kong-plugins/`)
**Rationale**: Proprietary chat tracking

**Files to Move**:
```
/agent-gateway/plugins/chat-logger/   → enterprise/kong-plugins/chat-logger/
```

### 2. OPEN SOURCE COMPONENTS (nasiko-core)

#### 2.1 Core Backend (`nasiko-core/app/`)
**Components**:
- Agent registry (CRUD operations)
- Agent upload (GitHub, ZIP, directory)
- Agent operations (build, deploy, lifecycle)
- GitHub integration
- N8N integration
- Chat history
- Search
- Health checks
- Basic routing (keyword/tag-based)

**Removed/Modified Files**:
- Remove: `/app/api/auth.py`
- Remove: `/app/pkg/auth/`
- Remove: `/app/api/routes/superuser_routes.py`
- Modify: `/app/api/routes/router.py` - Remove auth dependencies
- Modify: `/app/service/service.py` - Remove auth_client calls
- Add: `/app/api/routes/router_routes.py` - Simple community router

**Auth Handling in Community**:
```python
# nasiko-core/app/pkg/auth/simple_auth.py (optional basic auth)
async def get_current_user_optional():
    """No-op auth for community edition"""
    return None

# Or implement simple API key auth without ACL
```

#### 2.2 Core Frontend (`nasiko-core/web/`)
**Components**:
- Agent browsing and search
- Agent upload UI
- GitHub/N8N integration screens
- Query submission (using basic router)
- Observability/traces
- Login screen (if simple auth enabled)

**Removed Files**:
- Remove: `/web/lib/presentation/access_control/`
- Remove: `/web/lib/data/repositories/access_control_repository_impl.dart`
- Remove: `/web/lib/data/datasources/access_control_data_source.dart`
- Remove: `/web/lib/data/models/agent_permissions_model.dart`
- Remove: `/web/lib/data/models/user_model.dart`
- Modify: Navigation to hide ACL routes

#### 2.3 Core CLI (`nasiko-core/cli/`)
**Components**:
- Login/logout (simple auth)
- Agent operations (upload, list, details)
- GitHub operations
- N8N integration
- Chat history
- Search
- Observability
- Infrastructure setup (K8s, registry, buildkit)

**Removed Files**:
- Remove: `/cli/groups/access_group.py`
- Remove: `/cli/commands/access.py`
- Remove: `/cli/commands/user_management.py`

#### 2.4 Orchestrator (`nasiko-core/orchestrator/`)
**Keep All Files** - Infrastructure setup is open source:
- Docker network setup
- Service management
- Agent builder
- Instrumentation injector
- Redis stream listener
- Registry manager

**Modifications**:
- Conditional service loading (don't start auth service in community edition)
- Conditional superuser setup (skip in community)

#### 2.5 Kong Gateway (`nasiko-core/agent-gateway/`)
**Keep Core Files**:
- Registry service discovery (`/agent-gateway/registry/registry.py`)
- Docker compose setup
- Start scripts

**Remove**:
- Chat logger plugin (move to enterprise)

#### 2.6 Sample Agents (`nasiko-core/agents/`)
**Keep All** - These are examples for open source community

#### 2.7 Infrastructure (`nasiko-core/cli/setup/`, `/k8s/`, `/terraform/`)
**Keep All** - Infrastructure as code is open source

**Modifications**:
- Make auth service deployment conditional
- Make router service deployment conditional (deploy basic router instead)

#### 2.8 Documentation (`nasiko-core/docs/`)
**Keep Core Docs, Add Edition Notes**:
- API.md - Remove auth service endpoints, add note about enterprise edition
- ARCHITECTURE.md - Describe plugin architecture
- SETUP.md - Add instructions for both editions
- Add: ENTERPRISE.md - Feature comparison and upgrade guide

---

## Implementation Steps

### Phase 1: Prepare Plugin Architecture (Week 1-2)

#### 1.1 Create Plugin Interfaces
```python
# nasiko-core/app/pkg/plugins/auth_plugin.py
from abc import ABC, abstractmethod

class AuthPlugin(ABC):
    @abstractmethod
    async def validate_token(self, token: str) -> dict:
        pass

    @abstractmethod
    async def get_current_user(self) -> dict:
        pass

    @abstractmethod
    async def create_agent_permissions(self, agent_id: str, owner_id: str):
        pass

class NoAuthPlugin(AuthPlugin):
    """Default no-op auth for community edition"""
    async def validate_token(self, token: str):
        return {"valid": True, "user_id": "anonymous"}

    async def get_current_user(self):
        return None

    async def create_agent_permissions(self, agent_id: str, owner_id: str):
        pass  # No-op
```

```python
# nasiko-core/app/pkg/plugins/router_plugin.py
from abc import ABC, abstractmethod

class RouterPlugin(ABC):
    @abstractmethod
    async def route_query(self, query: str, context: dict) -> dict:
        pass

class BasicRouter(RouterPlugin):
    """Simple keyword-based router for community edition"""
    async def route_query(self, query: str, context: dict):
        # Simple tag matching logic
        pass
```

#### 1.2 Create Plugin Loader
```python
# nasiko-core/app/pkg/plugins/loader.py
import os
import importlib

class PluginLoader:
    def __init__(self):
        self.edition = os.getenv("NASIKO_EDITION", "community")
        self.auth_plugin = None
        self.router_plugin = None

    def load_auth_plugin(self):
        if self.edition == "enterprise":
            try:
                module = importlib.import_module("enterprise.backend_plugins.auth.plugin")
                self.auth_plugin = module.EnterpriseAuthPlugin()
            except ImportError:
                raise RuntimeError("Enterprise edition configured but plugins not found")
        else:
            from app.pkg.plugins.auth_plugin import NoAuthPlugin
            self.auth_plugin = NoAuthPlugin()
        return self.auth_plugin

    def load_router_plugin(self):
        if self.edition == "enterprise":
            try:
                module = importlib.import_module("enterprise.router_plugin.plugin")
                self.router_plugin = module.EnterpriseRouterPlugin()
            except ImportError:
                raise RuntimeError("Enterprise edition configured but plugins not found")
        else:
            from app.pkg.plugins.router_plugin import BasicRouter
            self.router_plugin = BasicRouter()
        return self.router_plugin

# Global plugin loader
plugin_loader = PluginLoader()
```

#### 1.3 Update Main App
```python
# nasiko-core/app/main.py
from app.pkg.plugins.loader import plugin_loader

# Load plugins on startup
@app.on_event("startup")
async def startup_event():
    auth_plugin = plugin_loader.load_auth_plugin()
    router_plugin = plugin_loader.load_router_plugin()

    # Store in app state for access in routes
    app.state.auth_plugin = auth_plugin
    app.state.router_plugin = router_plugin

    # Conditionally include enterprise routes
    if plugin_loader.edition == "enterprise":
        from enterprise.backend_plugins.auth.routes import router as auth_router
        app.include_router(auth_router, prefix="/api/v1/auth")
```

### Phase 2: Extract Proprietary Components (Week 3-4)

#### 2.1 Create Enterprise Repository Structure
```bash
mkdir -p nasiko-enterprise/enterprise/{auth-service,router,cli-extensions,web-extensions,backend-plugins,kong-plugins}
```

#### 2.2 Move Auth Service
```bash
# Move entire auth-service directory
mv auth-service nasiko-enterprise/enterprise/

# Create plugin adapter
cat > nasiko-enterprise/enterprise/backend-plugins/auth/plugin.py << 'EOF'
from app.pkg.plugins.auth_plugin import AuthPlugin
from enterprise.auth_service.service.auth_service import AuthService

class EnterpriseAuthPlugin(AuthPlugin):
    def __init__(self):
        self.auth_service = AuthService()

    async def validate_token(self, token: str):
        return await self.auth_service.validate_token(token)

    async def get_current_user(self):
        return await self.auth_service.get_current_user()

    async def create_agent_permissions(self, agent_id, owner_id):
        return await self.auth_service.create_agent_permissions(agent_id, owner_id)
EOF
```

#### 2.3 Move Router Service
```bash
mv agent-gateway/router nasiko-enterprise/enterprise/

# Create plugin adapter
cat > nasiko-enterprise/enterprise/router-plugin/plugin.py << 'EOF'
from app.pkg.plugins.router_plugin import RouterPlugin
from enterprise.router.src.services.router_orchestrator import RouterOrchestrator

class EnterpriseRouterPlugin(RouterPlugin):
    def __init__(self):
        self.orchestrator = RouterOrchestrator()

    async def route_query(self, query: str, context: dict):
        return await self.orchestrator.route(query, context)
EOF
```

#### 2.4 Move Frontend Extensions
```bash
# Move access control screens
mkdir -p nasiko-enterprise/enterprise/web-extensions/lib/presentation/
mv web/lib/presentation/access_control nasiko-enterprise/enterprise/web-extensions/lib/presentation/

# Move related repositories and datasources
mkdir -p nasiko-enterprise/enterprise/web-extensions/lib/data/{repositories,datasources,models}
mv web/lib/data/repositories/access_control_repository_impl.dart nasiko-enterprise/enterprise/web-extensions/lib/data/repositories/
mv web/lib/data/datasources/access_control_data_source.dart nasiko-enterprise/enterprise/web-extensions/lib/data/datasources/
mv web/lib/data/models/agent_permissions_model.dart nasiko-enterprise/enterprise/web-extensions/lib/data/models/
mv web/lib/data/models/user_model.dart nasiko-enterprise/enterprise/web-extensions/lib/data/models/
```

#### 2.5 Move CLI Extensions
```bash
mkdir -p nasiko-enterprise/enterprise/cli-extensions/{groups,commands}
mv cli/groups/access_group.py nasiko-enterprise/enterprise/cli-extensions/groups/
mv cli/commands/access.py nasiko-enterprise/enterprise/cli-extensions/commands/
mv cli/commands/user_management.py nasiko-enterprise/enterprise/cli-extensions/commands/
```

### Phase 3: Create Community Replacements (Week 5)

#### 3.1 Basic Router Implementation
```python
# nasiko-core/app/api/routes/router_routes.py
from fastapi import APIRouter, Depends
from app.pkg.plugins.loader import plugin_loader

router = APIRouter()

@router.post("/router")
async def route_query(query: str):
    """Basic routing endpoint for community edition"""
    router_plugin = plugin_loader.router_plugin
    result = await router_plugin.route_query(query, {})
    return result
```

```python
# nasiko-core/app/pkg/plugins/router_plugin.py
class BasicRouter(RouterPlugin):
    async def route_query(self, query: str, context: dict):
        """Simple keyword and tag-based routing"""
        from app.repository.registry_repository import RegistryRepository

        repo = RegistryRepository()
        agents = await repo.get_all_agents()

        # Simple keyword matching in agent descriptions
        query_lower = query.lower()
        scored_agents = []

        for agent in agents:
            score = 0
            # Match keywords in name
            if query_lower in agent.name.lower():
                score += 10

            # Match keywords in description
            if agent.description and query_lower in agent.description.lower():
                score += 5

            # Match tags
            if agent.tags:
                for tag in agent.tags:
                    if query_lower in tag.lower():
                        score += 3

            if score > 0:
                scored_agents.append((agent, score))

        # Sort by score
        scored_agents.sort(key=lambda x: x[1], reverse=True)

        if scored_agents:
            best_agent = scored_agents[0][0]
            return {
                "selected_agent": {
                    "id": best_agent.id,
                    "name": best_agent.name,
                    "url": best_agent.service_url
                },
                "confidence": scored_agents[0][1] / 10.0
            }
        else:
            return {
                "selected_agent": None,
                "confidence": 0.0,
                "message": "No matching agent found"
            }
```

#### 3.2 Remove Auth Dependencies
```bash
# Remove auth files from core
rm -f app/api/auth.py
rm -rf app/pkg/auth/
rm -f app/api/routes/superuser_routes.py

# Update imports across codebase
# Replace all instances of:
#   from app.api.auth import get_current_user
# With:
#   from app.pkg.plugins.loader import plugin_loader
#   current_user = await plugin_loader.auth_plugin.get_current_user()
```

#### 3.3 Update Frontend
```dart
// nasiko-core/web/lib/core/config.dart
class AppConfig {
  static const String edition = String.fromEnvironment(
    'NASIKO_EDITION',
    defaultValue: 'community',
  );

  static bool get isEnterprise => edition == 'enterprise';
  static bool get isCommunity => edition == 'community';
}

// nasiko-core/web/lib/app/router.dart
import 'package:nasiko/core/config.dart';

final routes = <String, WidgetBuilder>{
  '/': (context) => HomeScreen(),
  '/agents': (context) => AgentsScreen(),
  '/upload': (context) => UploadScreen(),
  // ... core routes
};

// Conditionally add enterprise routes at runtime
if (AppConfig.isEnterprise) {
  // Import is conditional - will fail gracefully if not available
  try {
    final enterpriseRoutes = loadEnterpriseRoutes();
    routes.addAll(enterpriseRoutes);
  } catch (e) {
    print('Enterprise routes not available');
  }
}
```

### Phase 4: Setup Subtree (Week 6)

#### 4.1 Create Open Source Repository
```bash
# Create new repository on GitHub
gh repo create nasiko-io/nasiko-core --public

# Push open source code
cd nasiko-core
git init
git add .
git commit -m "Initial open source release"
git remote add origin https://github.com/nasiko-io/nasiko-core.git
git push -u origin main
```

#### 4.2 Setup Subtree in Enterprise Repo
```bash
# In existing nasiko repository (now nasiko-enterprise)
git remote add nasiko-core https://github.com/nasiko-io/nasiko-core.git
git subtree add --prefix=nasiko-core nasiko-core main --squash

# Future updates from open source
git subtree pull --prefix=nasiko-core nasiko-core main --squash

# Push changes back to open source
git subtree push --prefix=nasiko-core nasiko-core main
```

#### 4.3 Update Enterprise Docker Compose
```yaml
# nasiko-enterprise/docker-compose.yml
version: '3.8'

services:
  # Include community services
  backend:
    build:
      context: ./nasiko-core/app
      dockerfile: Dockerfile
    environment:
      - NASIKO_EDITION=enterprise  # Enable enterprise features
    volumes:
      - ./enterprise:/app/enterprise  # Mount enterprise plugins
    env_file:
      - .env

  # Enterprise-specific services
  auth-service:
    build:
      context: ./enterprise/auth-service
      dockerfile: Dockerfile
    ports:
      - "8001:8001"
    environment:
      - MONGO_URI=${MONGO_URI}
      - REDIS_HOST=redis
    depends_on:
      - mongodb
      - redis

  enterprise-router:
    build:
      context: ./enterprise/router
      dockerfile: Dockerfile
    ports:
      - "8005:8005"
    environment:
      - OPENROUTER_API_KEY=${OPENROUTER_API_KEY}
    depends_on:
      - backend
```

### Phase 5: Documentation & Testing (Week 7-8)

#### 5.1 Create Feature Comparison Matrix
```markdown
# nasiko-core/docs/ENTERPRISE.md

## Feature Comparison

| Feature | Community Edition | Enterprise Edition |
|---------|------------------|-------------------|
| **Agent Management** |
| Agent Registry | ✅ Full CRUD | ✅ Full CRUD |
| Agent Upload (GitHub/ZIP) | ✅ Yes | ✅ Yes |
| Build & Deploy | ✅ Kubernetes | ✅ Kubernetes |
| Kong Gateway | ✅ Yes | ✅ Yes |
| **Routing** |
| Basic Routing | ✅ Keyword/Tag matching | ❌ N/A |
| AI-Powered Router | ❌ No | ✅ FAISS + LLM |
| Semantic Search | ❌ No | ✅ Yes |
| **Authentication & Access Control** |
| Authentication | ⚠️ Optional simple auth | ✅ JWT tokens |
| User Management | ❌ No | ✅ Full CRUD |
| Agent Permissions | ❌ No | ✅ Owner + ACLs |
| Super User | ❌ No | ✅ Yes |
| API Access Control | ❌ No | ✅ User/Agent ACLs |
| **Observability** |
| LangTrace Integration | ✅ Yes | ✅ Yes |
| OpenTelemetry | ✅ Yes | ✅ Yes |
| Chat Logging | ❌ No | ✅ Kong plugin |
| **Integrations** |
| GitHub | ✅ Repository clone | ✅ Full OAuth |
| N8N | ✅ Yes | ✅ Yes |
| **Infrastructure** |
| Docker Compose | ✅ Yes | ✅ Yes |
| Kubernetes | ✅ Yes | ✅ Yes |
| Terraform | ✅ AWS/DO | ✅ AWS/DO |
| **CLI** |
| Agent Operations | ✅ Yes | ✅ Yes |
| User Management | ❌ No | ✅ Yes |
| Access Control | ❌ No | ✅ Grant/Revoke |
| **Support** |
| Community Support | ✅ GitHub Issues | ✅ GitHub Issues |
| Priority Support | ❌ No | ✅ Email/Slack |
| **License** |
| License | Apache 2.0 | Proprietary |
```

#### 5.2 Update Setup Instructions
```markdown
# nasiko-core/docs/SETUP.md

## Installation

### Community Edition (Open Source)

1. Clone repository:
   ```bash
   git clone https://github.com/nasiko-io/nasiko-core.git
   cd nasiko-core
   ```

2. Configure environment:
   ```bash
   cp .env.example .env
   # Edit .env and set:
   # NASIKO_EDITION=community
   ```

3. Start services:
   ```bash
   make start-nasiko
   ```

### Enterprise Edition

Contact sales@nasiko.io for access to enterprise features.

1. Clone enterprise repository:
   ```bash
   git clone https://github.com/your-org/nasiko-enterprise.git
   cd nasiko-enterprise
   ```

2. Configure environment:
   ```bash
   cp .env.enterprise.example .env
   # Edit .env and set:
   # NASIKO_EDITION=enterprise
   # Add enterprise-specific vars (OPENROUTER_API_KEY, etc.)
   ```

3. Start services:
   ```bash
   docker-compose -f docker-compose.enterprise.yml up
   ```
```

#### 5.3 Create Migration Guide
```markdown
# nasiko-enterprise/docs/MIGRATION_FROM_COMMUNITY.md

## Migrating from Community to Enterprise

### Step 1: Database Migration

Enterprise edition adds new collections for users and permissions:

```bash
# Run migration script
uv run enterprise/scripts/migrate_from_community.py
```

This script will:
1. Create `users` collection with default superuser
2. Create `agent_permissions` collection
3. Assign all existing agents to superuser as owner
4. Create default permissions

### Step 2: Update Configuration

Add enterprise environment variables:
```env
NASIKO_EDITION=enterprise
AUTH_SERVICE_URL=http://auth-service:8001
ROUTER_SERVICE_URL=http://enterprise-router:8005
OPENROUTER_API_KEY=your_key_here
JWT_SECRET=your_secret_here
```

### Step 3: Start Enterprise Services

```bash
docker-compose -f docker-compose.enterprise.yml up
```

### Step 4: Create Users

```bash
# Login as superuser (credentials from migration output)
nasiko login

# Create new users
nasiko users register --username alice --email alice@example.com

# Grant agent access
nasiko access grant-user --agent-id <agent_id> --user-ids <user_id>
```
```

#### 5.4 Testing Strategy
```markdown
# Testing Plan

## Community Edition Tests
- [ ] Agent upload (GitHub, ZIP, directory)
- [ ] Agent build and deploy
- [ ] Basic routing (keyword matching)
- [ ] Agent registry CRUD
- [ ] Kong gateway auto-discovery
- [ ] Observability (LangTrace traces)
- [ ] CLI commands (upload, list, search)
- [ ] Frontend (agent browsing, upload, query)

## Enterprise Edition Tests
- [ ] All community tests pass
- [ ] User registration and authentication
- [ ] JWT token validation
- [ ] Agent permission management
- [ ] Access control enforcement (users, agents)
- [ ] Super user privileges
- [ ] AI-powered routing (FAISS + LLM)
- [ ] Chat logging
- [ ] CLI access control commands
- [ ] Frontend ACL screens

## Plugin Architecture Tests
- [ ] Plugin loading (community vs enterprise)
- [ ] Graceful degradation (enterprise features in community)
- [ ] Configuration switching (NASIKO_EDITION env var)
- [ ] Import errors handled gracefully
```

---

## File Migration Checklist

### Files to MOVE to Enterprise

#### Auth Service
- [ ] `/auth-service/` → `enterprise/auth-service/`

#### Router Service
- [ ] `/agent-gateway/router/` → `enterprise/router/`

#### Backend Extensions
- [ ] `/app/api/auth.py` → `enterprise/backend-plugins/auth/middleware.py`
- [ ] `/app/pkg/auth/auth_client.py` → `enterprise/backend-plugins/auth/client.py`
- [ ] `/app/api/routes/superuser_routes.py` → `enterprise/backend-plugins/auth/routes.py`

#### CLI Extensions
- [ ] `/cli/groups/access_group.py` → `enterprise/cli-extensions/groups/`
- [ ] `/cli/commands/access.py` → `enterprise/cli-extensions/commands/`
- [ ] `/cli/commands/user_management.py` → `enterprise/cli-extensions/commands/`

#### Frontend Extensions
- [ ] `/web/lib/presentation/access_control/` → `enterprise/web-extensions/lib/presentation/`
- [ ] `/web/lib/data/repositories/access_control_repository_impl.dart` → `enterprise/web-extensions/lib/data/repositories/`
- [ ] `/web/lib/data/datasources/access_control_data_source.dart` → `enterprise/web-extensions/lib/data/datasources/`
- [ ] `/web/lib/data/models/agent_permissions_model.dart` → `enterprise/web-extensions/lib/data/models/`
- [ ] `/web/lib/data/models/user_model.dart` → `enterprise/web-extensions/lib/data/models/`

#### Kong Plugins
- [ ] `/agent-gateway/plugins/chat-logger/` → `enterprise/kong-plugins/chat-logger/`

### Files to KEEP in Community (with modifications)

#### Backend
- [ ] `/app/` (remove auth imports, add plugin system)
- [ ] `/app/api/routes/router.py` (update to use router plugin)
- [ ] Add: `/app/pkg/plugins/` (plugin interfaces and loader)
- [ ] Add: `/app/api/routes/router_routes.py` (basic routing endpoint)

#### Frontend
- [ ] `/web/` (remove access control screens, add config check)
- [ ] Update: `/web/lib/main.dart` (conditional route loading)

#### CLI
- [ ] `/cli/` (remove access commands, add conditional loading)
- [ ] Update: `/cli/main.py` (plugin-based command loading)

#### Orchestrator
- [ ] `/orchestrator/` (all files, add conditional service loading)

#### Kong Gateway
- [ ] `/agent-gateway/` (except router and chat-logger plugin)

#### Infrastructure
- [ ] `/k8s/`
- [ ] `/terraform/`
- [ ] `/cli/setup/`

#### Sample Agents
- [ ] `/agents/`

#### Docs
- [ ] `/docs/` (update with edition notes)

---

## Configuration Examples

### Community Edition `.env`
```env
NASIKO_EDITION=community

# Database
MONGO_URI=mongodb://admin:password@localhost:27017/nasiko
REDIS_HOST=localhost
REDIS_PORT=6379

# OpenAI for basic capabilities (optional)
OPENAI_API_KEY=sk-proj-...

# Observability
LANGTRACE_BASE_URL=http://localhost:3000

# Infrastructure
BUILDKIT_ADDRESS=tcp://buildkitd.buildkit.svc.cluster.local:1234
REGISTRY_URL=registry.digitalocean.com/nasiko-images
DO_TOKEN=your_do_token
```

### Enterprise Edition `.env`
```env
NASIKO_EDITION=enterprise

# All community vars plus:

# Auth Service
AUTH_SERVICE_URL=http://auth-service:8001
JWT_SECRET=your-super-secret-jwt-key-change-this
JWT_ALGORITHM=HS256
JWT_EXPIRATION_HOURS=12

# Enterprise Router
ROUTER_SERVICE_URL=http://enterprise-router:8005
OPENROUTER_API_KEY=your_openrouter_key
ROUTER_MODEL=google/gemini-2.5-flash

# Kong Chat Logger
KONG_ADMIN_URL=http://kong:8001
```

---

## Python Package Structure

### Community Edition (`pyproject.toml`)
```toml
[project]
name = "nasiko-core"
version = "0.1.0"
description = "Open source AI agent registry and orchestration platform"
license = { text = "Apache-2.0" }

dependencies = [
    "fastapi>=0.104.1",
    "pymongo>=4.6.0",
    "redis>=5.0.1",
    "kubernetes>=28.1.0",
    "pydantic>=2.5.0",
    "pydantic-settings>=2.1.0",
    # No LangChain, no FAISS (for router)
]
```

### Enterprise Edition (`pyproject.toml`)
```toml
[project]
name = "nasiko-enterprise"
version = "0.1.0"
description = "Nasiko Enterprise with advanced features"
license = { text = "Proprietary" }

dependencies = [
    "nasiko-core",  # Could install from PyPI or local path
    "langchain>=0.1.0",
    "langchain-openai>=0.0.2",
    "faiss-cpu>=1.7.4",
    "bcrypt>=4.1.0",  # For password hashing
    "PyJWT>=2.8.0",   # For JWT tokens
]
```

---

## Recommended Timeline

| Phase | Duration | Tasks |
|-------|----------|-------|
| **Phase 1** | 2 weeks | Design and implement plugin architecture |
| **Phase 2** | 2 weeks | Extract proprietary components to enterprise repo |
| **Phase 3** | 1 week | Create community replacements (basic router, no-op auth) |
| **Phase 4** | 1 week | Setup git subtree, configure repositories |
| **Phase 5** | 2 weeks | Documentation, testing, migration tools |
| **Total** | **8 weeks** | Complete separation with testing |

---

## Risks & Mitigations

### Risk 1: Breaking Changes During Separation
**Mitigation**:
- Create comprehensive test suite before starting
- Use feature flags to gradually transition
- Keep working branch until fully tested

### Risk 2: Accidental Exposure of Proprietary Code
**Mitigation**:
- Strict `.gitignore` in community repo
- Pre-commit hooks to prevent enterprise imports
- Code review checklist for OSS contributions

### Risk 3: Plugin System Overhead
**Mitigation**:
- Keep plugin interfaces minimal
- Use lazy loading for enterprise features
- Document plugin API clearly

### Risk 4: Maintenance Burden (Two Repos)
**Mitigation**:
- Use git subtree (not submodule) for easier syncing
- Automate subtree push/pull with GitHub Actions
- Establish clear contribution workflow

---

## Next Steps

1. **Review this plan** with team and get buy-in
2. **Create feature branch** `opensource-separation` in current repo
3. **Implement plugin architecture** (Phase 1) - can be done incrementally
4. **Set up test infrastructure** to ensure no regressions
5. **Extract components** one at a time (Phase 2)
6. **Create nasiko-core repository** and setup subtree
7. **Document everything** and create migration guides
8. **Soft launch** community edition with beta testers

---

## Questions to Address

1. **Licensing**: Which Apache/MIT license for community? Proprietary EULA for enterprise?
2. **Branding**: Same brand or different names? (e.g., "Nasiko Community" vs "Nasiko Enterprise")
3. **Support Model**: GitHub issues for community, dedicated support for enterprise?
4. **Distribution**: PyPI for community, private package registry for enterprise?
5. **Versioning**: Synchronized versions or independent?
6. **Contributions**: Accept PRs to community? How to handle enterprise features in PRs?

---

This separation plan allows you to maintain a single development workflow while providing both open-source and proprietary versions. The plugin architecture ensures clean boundaries and makes it easy to toggle features via configuration.
