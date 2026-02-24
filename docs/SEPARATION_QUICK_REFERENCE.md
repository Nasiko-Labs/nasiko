# Open Source Separation - Quick Reference

## TL;DR

**Goal**: Split Nasiko into open-source (nasiko-core) and proprietary (nasiko-enterprise) using a plugin-based architecture with git subtree.

**Strategy**: Open source = entry point, Enterprise = OSS + proprietary plugins loaded via `NASIKO_EDITION` env var

---

## What Goes Where?

### 🔓 Open Source (nasiko-core)
**Core Platform Features**:
- ✅ Agent registry (CRUD)
- ✅ Agent upload (GitHub/ZIP/directory)
- ✅ Agent build & deploy (K8s + BuildKit)
- ✅ Kong gateway + service discovery
- ✅ Basic routing (keyword/tag matching)
- ✅ GitHub integration (clone repos)
- ✅ N8N integration
- ✅ Observability (LangTrace + OTEL)
- ✅ Infrastructure setup (Terraform, K8s, Helm)
- ✅ Sample agents
- ✅ CLI (core commands)
- ✅ Frontend (core UI)
- ✅ Orchestrator

**Components**:
```
nasiko-core/
├── app/                 # Backend (minus auth)
├── web/                 # Frontend (minus ACL UI)
├── cli/                 # CLI (minus user/access commands)
├── orchestrator/        # Deployment orchestration
├── agent-gateway/       # Kong + basic discovery
├── agents/              # Sample agents
├── k8s/                 # Kubernetes manifests
├── terraform/           # IaC
└── docs/                # Documentation
```

### 🔒 Proprietary (nasiko-enterprise)

**Premium Features**:
- 🔐 Auth service (JWT tokens, user management)
- 🔐 Access control (agent permissions, ACLs)
- 🚀 AI-powered router (FAISS + LLM)
- 📊 Chat logging (Kong plugin)
- 👥 User management UI
- 🔑 CLI access control commands

**Components**:
```
nasiko-enterprise/
├── nasiko-core/                    # Git subtree (OSS)
├── enterprise/
│   ├── auth-service/               # Full auth microservice
│   ├── router/                     # AI routing engine
│   ├── backend-plugins/auth/       # Auth middleware
│   ├── cli-extensions/             # Access control CLI
│   ├── web-extensions/             # ACL UI components
│   └── kong-plugins/chat-logger/   # Chat tracking
└── docker-compose.enterprise.yml
```

---

## Plugin Architecture

### How It Works

```python
# 1. Environment variable controls edition
NASIKO_EDITION=community  # or "enterprise"

# 2. Plugin loader dynamically imports features
if NASIKO_EDITION == "enterprise":
    auth_plugin = EnterpriseAuthPlugin()  # Full JWT + ACL
    router_plugin = EnterpriseRouterPlugin()  # AI routing
else:
    auth_plugin = NoAuthPlugin()  # No-op
    router_plugin = BasicRouter()  # Keyword matching
```

### Plugin Interfaces

```python
# app/pkg/plugins/auth_plugin.py
class AuthPlugin(ABC):
    @abstractmethod
    async def validate_token(self, token: str) -> dict: pass

    @abstractmethod
    async def get_current_user(self) -> dict: pass

# app/pkg/plugins/router_plugin.py
class RouterPlugin(ABC):
    @abstractmethod
    async def route_query(self, query: str, context: dict) -> dict: pass
```

---

## File Migration Map

### Move to Enterprise

| Source | Destination |
|--------|-------------|
| `/auth-service/*` | `enterprise/auth-service/` |
| `/agent-gateway/router/*` | `enterprise/router/` |
| `/app/api/auth.py` | `enterprise/backend-plugins/auth/middleware.py` |
| `/app/pkg/auth/*` | `enterprise/backend-plugins/auth/` |
| `/cli/commands/access.py` | `enterprise/cli-extensions/commands/` |
| `/cli/commands/user_management.py` | `enterprise/cli-extensions/commands/` |
| `/web/lib/presentation/access_control/*` | `enterprise/web-extensions/lib/presentation/` |
| `/agent-gateway/plugins/chat-logger/*` | `enterprise/kong-plugins/chat-logger/` |

### Keep in Community (Modify)

| File | Changes |
|------|---------|
| `/app/main.py` | Add plugin loader, conditional route inclusion |
| `/app/api/routes/router.py` | Use router plugin instead of direct router service |
| `/cli/main.py` | Conditional command group loading |
| `/web/lib/main.dart` | Conditional route loading for ACL screens |
| `/orchestrator/orchestrator.py` | Skip auth-service startup in community |

### Create New in Community

| File | Purpose |
|------|---------|
| `/app/pkg/plugins/loader.py` | Plugin loader |
| `/app/pkg/plugins/auth_plugin.py` | Auth interface + NoAuthPlugin |
| `/app/pkg/plugins/router_plugin.py` | Router interface + BasicRouter |
| `/app/api/routes/router_routes.py` | Basic routing endpoint |

---

## Git Subtree Commands

### Initial Setup
```bash
# 1. Create nasiko-core repo (new public repo)
git init nasiko-core
cd nasiko-core
# ... copy community files ...
git commit -m "Initial open source release"
git remote add origin https://github.com/nasiko-io/nasiko-core.git
git push -u origin main

# 2. In existing nasiko repo (becomes enterprise)
git remote add nasiko-core https://github.com/nasiko-io/nasiko-core.git
git subtree add --prefix=nasiko-core nasiko-core main --squash
```

### Ongoing Sync
```bash
# Pull updates from community into enterprise
git subtree pull --prefix=nasiko-core nasiko-core main --squash

# Push changes from enterprise back to community
git subtree push --prefix=nasiko-core nasiko-core main
```

---

## Configuration

### Community `.env`
```env
NASIKO_EDITION=community
MONGO_URI=mongodb://admin:password@localhost:27017/nasiko
REDIS_HOST=localhost
OPENAI_API_KEY=sk-proj-...  # Optional, for basic features
LANGTRACE_BASE_URL=http://localhost:3000
```

### Enterprise `.env`
```env
NASIKO_EDITION=enterprise

# All community vars PLUS:
AUTH_SERVICE_URL=http://auth-service:8001
JWT_SECRET=your-secret-key
ROUTER_SERVICE_URL=http://enterprise-router:8005
OPENROUTER_API_KEY=your_openrouter_key
ROUTER_MODEL=google/gemini-2.5-flash
```

---

## Feature Matrix

| Feature | Community | Enterprise |
|---------|-----------|------------|
| Agent Registry | ✅ | ✅ |
| Agent Upload | ✅ | ✅ |
| Build & Deploy | ✅ | ✅ |
| Kong Gateway | ✅ | ✅ |
| **Routing** | Keyword matching | AI (FAISS + LLM) |
| **Auth** | None/Simple | JWT tokens |
| **User Management** | ❌ | ✅ |
| **Agent ACLs** | ❌ | ✅ Owner + permissions |
| **Chat Logging** | ❌ | ✅ Kong plugin |
| **Support** | GitHub Issues | Priority |
| **License** | Apache 2.0 | Proprietary |

---

## Implementation Phases

### Phase 1: Plugin Architecture (2 weeks)
- [ ] Create plugin interfaces (auth, router)
- [ ] Implement plugin loader
- [ ] Add NoAuthPlugin and BasicRouter
- [ ] Update main.py to use plugins

### Phase 2: Extract Components (2 weeks)
- [ ] Move auth-service to enterprise/
- [ ] Move router to enterprise/
- [ ] Move CLI/web extensions
- [ ] Create plugin adapters

### Phase 3: Community Replacements (1 week)
- [ ] Implement BasicRouter
- [ ] Remove auth dependencies
- [ ] Update frontend for conditional features
- [ ] Update CLI for conditional commands

### Phase 4: Repository Setup (1 week)
- [ ] Create nasiko-core repo
- [ ] Setup git subtree
- [ ] Configure CI/CD
- [ ] Update .gitignore

### Phase 5: Documentation & Testing (2 weeks)
- [ ] Write migration guide
- [ ] Create feature comparison docs
- [ ] Test both editions
- [ ] Create upgrade path

---

## Critical Success Factors

1. **Complete Test Coverage**: Test suite must pass for both editions
2. **Clear Plugin Boundaries**: No enterprise code in community repo
3. **Documentation**: Migration guide and setup docs for both editions
4. **Graceful Degradation**: Enterprise features fail gracefully in community
5. **Git Subtree Automation**: CI/CD to sync repos automatically

---

## Quick Start Commands

### Run Community Edition
```bash
git clone https://github.com/nasiko-io/nasiko-core.git
cd nasiko-core
cp .env.example .env
# Set NASIKO_EDITION=community
make start-nasiko
```

### Run Enterprise Edition
```bash
git clone https://github.com/your-org/nasiko-enterprise.git
cd nasiko-enterprise
cp .env.enterprise.example .env
# Set NASIKO_EDITION=enterprise
docker-compose -f docker-compose.enterprise.yml up
```

---

## Support & Questions

- **Community**: GitHub Issues at `nasiko-io/nasiko-core`
- **Enterprise**: Contact sales@nasiko.io
- **Docs**: See `/docs/OPEN_SOURCE_SEPARATION_PLAN.md` for full details

---

## Key Decisions Needed

1. ⚠️ **License**: Apache 2.0 or MIT for community?
2. ⚠️ **Branding**: Same name or "Nasiko Community" vs "Nasiko Enterprise"?
3. ⚠️ **Versioning**: Synchronized or independent version numbers?
4. ⚠️ **Distribution**: Publish community to PyPI?
5. ⚠️ **Support Model**: GitHub-only for community or forums?
6. ⚠️ **Contribution Policy**: Accept PRs to community repo?
