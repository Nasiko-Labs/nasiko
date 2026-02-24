# Architecture Notes: Auth Service & ACL for OSS vs Enterprise

## Current Situation

The Nasiko ecosystem has two versions:
1. **nasiko-oss** (OpenSource): Single-tenant, no multi-user ACL needed
2. **nasiko** (Enterprise): Multi-tenant, complex ACL and authorization

Both currently reference the same auth-service from the parent repo, which creates unnecessary complexity for the OSS version.

## Architecture Challenge

### Current Issue
```
nasiko-oss/
  ├── app/ (FastAPI backend)
  │   ├── api/auth.py → Calls AUTH_SERVICE_URL (http://nasiko-auth-service:8001)
  │   ├── api/routes/superuser_routes.py → Calls AUTH_SERVICE_URL
  │   ├── api/handlers/registry_handler.py → Gets ACLs from auth service
  │   └── api/handlers/search_handler.py → Syncs users from auth service
  │
  ├── docker-compose.nasiko.yml → Includes nasiko-auth-service
  │
  └── cli/groups/local_group.py → No auth handling

nasiko/ (parent)
  ├── auth-service/ (Enterprise ACL service)
  │   └── Full multi-tenant auth + ACL logic
  └── core/ (submodule pointing to nasiko-oss)
```

### Problem
- OSS version drags in auth service complexity it doesn't need
- Both versions share same auth contract, forcing compatibility
- Single-user OSS deployment still validates tokens, manages users
- Unnecessary Redis+MongoDB overhead for auth in OSS

### Desired State
```
OSS Version (nasiko-oss/):
  - No auth validation middleware
  - No user/ACL database models
  - No auth service dependency
  - Simple bypass or single-token mode
  - Works standalone without auth-service

Enterprise Version (nasiko/):
  - Full auth service integration
  - Multi-tenant ACL
  - User management
  - Token validation
  - Uses auth-service for all auth decisions
  - Uses subtree (nasiko-oss/core/) as foundation
```

## Solution Architecture

### Option 1: Feature Flag Based (RECOMMENDED)

Use environment variable to toggle auth mode:

```python
# app/api/auth.py
AUTH_MODE = os.getenv("AUTH_MODE", "simple")  # "simple" or "enterprise"

async def validate_token_with_auth_service(token: str) -> dict:
    """Validate token - behavior depends on AUTH_MODE"""
    if AUTH_MODE == "simple":
        # OSS: Just check token exists, return default user
        if not token:
            raise HTTPException(status_code=401, detail="Token required")
        return {"user_id": "default", "user": "admin"}

    # Enterprise: Full validation via auth service
    response = await client.get(f"{AUTH_SERVICE_URL}/auth/validate")
    # ...
```

### Option 2: Service Abstraction

Create auth provider interface:

```python
# app/pkg/auth/provider.py
class AuthProvider(ABC):
    @abstractmethod
    async def validate_token(self, token: str) -> dict: pass
    @abstractmethod
    async def get_user(self, user_id: str) -> dict: pass
    @abstractmethod
    async def get_user_agents(self, user_id: str) -> List[str]: pass

# app/pkg/auth/simple_provider.py
class SimpleAuthProvider(AuthProvider):
    """No-op auth for OSS single-tenant"""
    async def validate_token(self, token: str) -> dict:
        return {"user_id": "default", "is_admin": True}

# app/pkg/auth/enterprise_provider.py
class EnterpriseAuthProvider(AuthProvider):
    """Full auth service integration for enterprise"""
    async def validate_token(self, token: str) -> dict:
        # Call auth service...
```

## Recommended Implementation: Option 1 (Feature Flag)

Simpler, less refactoring, backward compatible with enterprise.

### Changes Required

#### 1. Environment Variable
```bash
# .env.local.example
AUTH_MODE=simple              # "simple" for OSS, "enterprise" for multi-tenant
```

#### 2. docker-compose.nasiko.yml
```yaml
# For OSS (development)
# Skip nasiko-auth-service entirely when AUTH_MODE=simple

# In service environment
nasiko-backend:
  environment:
    AUTH_MODE: simple
    # AUTH_SERVICE_URL is optional when AUTH_MODE=simple
    AUTH_SERVICE_URL: ${AUTH_SERVICE_URL:-http://localhost:8082}
```

#### 3. app/api/auth.py
```python
import os

AUTH_MODE = os.getenv("AUTH_MODE", "enterprise")
AUTH_SERVICE_URL = os.getenv("AUTH_SERVICE_URL", "http://nasiko-auth-service:8001")

async def validate_token_with_auth_service(token: str) -> dict:
    """Validate token - behavior depends on AUTH_MODE"""

    if AUTH_MODE == "simple":
        # OSS Mode: No validation, single user
        logger.debug("Using simple auth mode (OSS)")
        if not token:
            raise HTTPException(status_code=401, detail="Token required")
        # Return default admin user
        return {
            "user_id": "default-user",
            "username": "admin",
            "email": "admin@localhost",
            "is_admin": True,
            "organizations": ["default"]
        }

    # Enterprise Mode: Full auth service validation
    logger.debug(f"Using enterprise auth mode, calling {AUTH_SERVICE_URL}")
    try:
        async with httpx.AsyncClient() as client:
            response = await client.get(
                f"{AUTH_SERVICE_URL}/auth/validate",
                headers={"Authorization": f"Bearer {token}"},
                timeout=10
            )
            if response.status_code == 200:
                return response.json()
            # ... error handling
    except Exception as e:
        logger.error(f"Auth service error: {e}")
        raise HTTPException(status_code=500, detail="Auth service unavailable")
```

#### 4. app/api/routes/superuser_routes.py
```python
AUTH_MODE = os.getenv("AUTH_MODE", "enterprise")

@router.post("/superuser/register-user")
async def register_user_endpoint(user_data: UserRegistration):
    """Register new user"""

    if AUTH_MODE == "simple":
        # OSS: Skip user registration (single user)
        logger.info("User registration skipped in simple auth mode")
        return {
            "status": "skipped",
            "message": "User management disabled in single-tenant mode"
        }

    # Enterprise: Call auth service
    async with httpx.AsyncClient() as client:
        response = await client.post(
            f"{AUTH_SERVICE_URL}/auth/users/register",
            json=user_data.dict()
        )
        # ...
```

#### 5. app/api/handlers/registry_handler.py
```python
AUTH_MODE = os.getenv("AUTH_MODE", "enterprise")

async def get_user_agents_list(self, user_id: str = "default"):
    """Get agents for user - OSS returns all, Enterprise checks ACL"""

    if AUTH_MODE == "simple":
        # OSS: Return all agents (single user has access to everything)
        logger.debug("Simple auth mode: returning all agents")
        agents = await self.repo.get_all_agents()
        return agents

    # Enterprise: Filter by user's ACL
    logger.debug(f"Enterprise mode: filtering agents for user {user_id}")
    user_agents = await self.get_user_accessible_agents(user_id)
    # ... filter logic
```

#### 6. app/api/handlers/search_handler.py
```python
AUTH_MODE = os.getenv("AUTH_MODE", "enterprise")

async def sync_users_from_auth_service(self):
    """Sync users for search indexing"""

    if AUTH_MODE == "simple":
        # OSS: Single default user, no sync needed
        logger.info("User sync skipped in simple auth mode")
        return

    # Enterprise: Sync from auth service
    auth_service_url = os.getenv("AUTH_SERVICE_URL", "http://nasiko-auth-service:8001")
    # ... sync logic
```

#### 7. Update docker-compose.nasiko.yml
```yaml
# Option A: Always include auth service (can be disabled via AUTH_MODE)
nasiko-auth-service:
  image: ${NASIKO_REGISTRY_URL:-nasiko}/nasiko-auth-service:${NASIKO_VERSION:-latest}
  container_name: nasiko-auth-service
  environment:
    AUTH_MODE: ${AUTH_MODE:-simple}
    # ... rest of config
  # Add condition: only start if AUTH_MODE=enterprise
  # Note: Docker Compose doesn't support conditional services, so we include it but disable in app

# Option B: Create separate compose for enterprise
# docker-compose.enterprise.yml (extends docker-compose.nasiko.yml with auth-service)
```

## Implementation Steps

### Phase 1: Add Feature Flag (Minimal Changes)

1. **Update env config**:
   - Add `AUTH_MODE` to `.env.local.example`
   - Document usage

2. **Update auth.py**:
   - Add `AUTH_MODE` check in `validate_token_with_auth_service()`
   - Simple auth returns default user
   - Enterprise auth calls service

3. **Test**:
   - Run with `AUTH_MODE=simple` - no auth service needed
   - Verify API works without auth-service running

### Phase 2: Update Endpoints (Progressive)

1. **registry_handler.py**: Check `AUTH_MODE` for agent list
2. **superuser_routes.py**: Skip user registration in simple mode
3. **search_handler.py**: Skip user sync in simple mode

### Phase 3: Update Compose Files (Optional)

1. **docker-compose.nasiko.yml** (for OSS):
   - Add `AUTH_MODE: simple` by default
   - Keep auth service for compatibility but disabled

2. **docker-compose.enterprise.yml** (for enterprise):
   - Extend OSS compose
   - Override `AUTH_MODE: enterprise`
   - Set auth service configuration

## Benefits

### For OSS Users
✅ No auth service installation needed
✅ Single command deployment: `nasiko local up`
✅ No user management overhead
✅ Faster startup (one less service)
✅ Standalone - works without parent repo

### For Enterprise Users
✅ Full multi-tenant auth
✅ ACL-based access control
✅ User management UI
✅ Audit logging
✅ Uses OSS as foundation via subtree

### For Maintainers
✅ Single codebase supports both versions
✅ No code duplication
✅ Easy feature flag toggles
✅ Clear separation of concerns
✅ Enterprise extends OSS naturally

## Backward Compatibility

✅ Existing code continues to work
✅ Auth service still accessible if needed
✅ `AUTH_MODE=enterprise` is default-compatible
✅ No breaking changes to API contracts

## Configuration Examples

### OSS Development
```bash
# .env
AUTH_MODE=simple
DOCKER_NETWORK=nasiko-network
NASIKO_VERSION=latest
```

```bash
nasiko local up
# Auth service NOT needed
```

### Enterprise Development
```bash
# .env
AUTH_MODE=enterprise
AUTH_SERVICE_URL=http://nasiko-auth-service:8001
DOCKER_NETWORK=nasiko-network
```

```bash
nasiko local up
# Includes auth service
# Full ACL support
```

### Production Enterprise
```bash
# Kubernetes setup with ACL
AUTH_MODE=enterprise
AUTH_SERVICE_URL=https://auth.enterprise.company.com
# ... full enterprise config
```

## Default Behavior

| Aspect | OSS (AUTH_MODE=simple) | Enterprise (AUTH_MODE=enterprise) |
|--------|------------------------|----------------------------------|
| Token Validation | Accepts any token | Validates via auth service |
| User Management | Single default user | Multi-tenant with ACL |
| ACL Enforcement | All agents visible | User-specific filtered |
| Auth Service | Optional | Required |
| User Registration | Disabled | Enabled |
| Database | No auth DB | Full auth DB |

## Code Changes Summary

### New/Modified Files
```
app/api/auth.py
  ✏️  Add AUTH_MODE check
  ✏️  Conditional token validation

app/api/routes/superuser_routes.py
  ✏️  Add AUTH_MODE check
  ✏️  Skip user registration if simple

app/api/handlers/registry_handler.py
  ✏️  Add AUTH_MODE check
  ✏️  Return all agents in simple mode

app/api/handlers/search_handler.py
  ✏️  Add AUTH_MODE check
  ✏️  Skip user sync if simple

.env.local.example
  ✏️  Add AUTH_MODE=simple

docker-compose.nasiko.yml
  ✏️  Add AUTH_MODE env var
```

### No Changes Needed
```
✅ API contracts remain same
✅ Agent deployment logic unchanged
✅ Kong routing unchanged
✅ Orchestrator unchanged
✅ CLI unchanged
```

## Enterprise Subtree Usage

In parent repo (nasiko/):

```yaml
# docker-compose.yml (enterprise)
version: '3.8'

services:
  # ... all OSS services from docker-compose.nasiko.yml
  # (included via subtree reference)

  # Enterprise additions
  nasiko-auth-service:
    image: nasiko/auth-service:latest
    environment:
      AUTH_MODE: enterprise
      ACL_ENABLED: true
      RBAC_ENABLED: true
      # ... multi-tenant config
    depends_on:
      - mongodb
      - redis
```

## Risk Assessment

### Low Risk
- Feature flag is self-contained
- No breaking changes
- Backward compatible
- Easy to revert

### Testing Required
- [ ] OSS with AUTH_MODE=simple (no auth service)
- [ ] Enterprise with AUTH_MODE=enterprise (full auth)
- [ ] Token validation paths
- [ ] ACL filtering logic
- [ ] Docker Compose startup

## Deployment Timeline

1. **Week 1**: Implement feature flag in auth.py + env config
2. **Week 2**: Update endpoints (handlers, routes)
3. **Week 3**: Test OSS and Enterprise modes
4. **Week 4**: Create enterprise docker-compose.yml
5. **Week 5**: Documentation updates

## Documentation Updates Needed

- [ ] Add AUTH_MODE to `.env.local.example`
- [ ] Update `docs/LOCAL_DEVELOPMENT.md` (no auth setup needed)
- [ ] Create `docs/ENTERPRISE_SETUP.md` (auth configuration)
- [ ] Document auth modes in architecture doc
- [ ] Add troubleshooting for AUTH_MODE issues

## Conclusion

Using a feature flag approach for `AUTH_MODE` provides:
- ✅ Clean separation between OSS and Enterprise
- ✅ Minimal code changes
- ✅ Production-ready
- ✅ Maintainable and scalable
- ✅ Backward compatible
- ✅ Easy to document

This architecture allows nasiko-oss to be standalone and simple while enabling enterprise features through configuration, not code duplication.
