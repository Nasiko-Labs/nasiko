# AUTH_MODE Implementation - Copy-Paste Ready Code Snippets

This document contains production-ready code snippets ready to be integrated into the Nasiko codebase.

## Quick Navigation

- [File 1: app/api/auth.py](#file-1-appapiauthpy)
- [File 2: app/api/routes/superuser_routes.py](#file-2-appapirotessuperuser_routespy)
- [File 3: app/api/handlers/registry_handler.py](#file-3-appapihandlersregistry_handlerpy)
- [File 4: app/api/handlers/search_handler.py](#file-4-appapihandlerssearch_handlerpy)

---

## File 1: app/api/auth.py

### Location
Find the existing `validate_token_with_auth_service()` function in `app/api/auth.py`

### What to Replace
Look for:
```python
async def validate_token_with_auth_service(token: str) -> dict:
    """Validate token by calling auth service /auth/validate endpoint"""
    # ... existing implementation
```

### Replacement Code

```python
import os
import logging
import httpx
from fastapi import HTTPException

logger = logging.getLogger(__name__)

# Get AUTH_MODE from environment
AUTH_MODE = os.getenv("AUTH_MODE", "enterprise")
AUTH_SERVICE_URL = os.getenv("AUTH_SERVICE_URL", "http://nasiko-auth-service:8001")


async def validate_token_with_auth_service(token: str) -> dict:
    """
    Validate token - behavior depends on AUTH_MODE

    AUTH_MODE=simple (OSS):
      - No validation, single user
      - Returns default admin user
      - No auth service dependency

    AUTH_MODE=enterprise (Multi-tenant):
      - Validates via auth service
      - Full user profile returned
      - ACL-ready architecture

    Args:
        token: JWT token to validate

    Returns:
        dict: User info with user_id, username, email, etc.

    Raises:
        HTTPException: 401 if token invalid, 503 if service unavailable
    """

    # ==================== SIMPLE MODE (OSS) ====================
    if AUTH_MODE == "simple":
        logger.debug("Using simple auth mode (OSS single-tenant)")

        if not token:
            raise HTTPException(
                status_code=401,
                detail="Token required"
            )

        # Return default admin user for single-tenant mode
        logger.debug("Simple mode: returning default user (no validation)")
        return {
            "user_id": "default-user",
            "username": "admin",
            "email": "admin@localhost",
            "is_admin": True,
            "organizations": ["default"],
            "oid": "default-org-id",
            "auth_mode": "simple"
        }

    # ==================== ENTERPRISE MODE (Multi-tenant) ====================
    logger.debug(f"Using enterprise auth mode - calling {AUTH_SERVICE_URL}")

    if not token:
        raise HTTPException(
            status_code=401,
            detail="Token required"
        )

    try:
        async with httpx.AsyncClient() as client:
            response = await client.get(
                f"{AUTH_SERVICE_URL}/auth/validate",
                headers={"Authorization": f"Bearer {token}"},
                timeout=10
            )

            if response.status_code == 200:
                user_data = response.json()
                logger.info(f"Token validated for user: {user_data.get('user_id')}")
                return user_data

            elif response.status_code == 401:
                logger.warning(f"Token validation failed: invalid token")
                raise HTTPException(
                    status_code=401,
                    detail="Invalid or expired token"
                )

            elif response.status_code == 403:
                logger.warning(f"Token validation failed: forbidden")
                raise HTTPException(
                    status_code=403,
                    detail="Access forbidden"
                )

            else:
                logger.error(f"Auth service returned {response.status_code}")
                raise HTTPException(
                    status_code=response.status_code,
                    detail=f"Auth service error: {response.status_code}"
                )

    except httpx.ConnectError:
        logger.error(f"Cannot connect to auth service at {AUTH_SERVICE_URL}")
        raise HTTPException(
            status_code=503,
            detail="Auth service unavailable"
        )
    except httpx.TimeoutException:
        logger.error(f"Auth service timeout at {AUTH_SERVICE_URL}")
        raise HTTPException(
            status_code=504,
            detail="Auth service timeout"
        )
    except Exception as e:
        logger.error(f"Unexpected error validating token: {type(e).__name__}: {e}")
        raise HTTPException(
            status_code=500,
            detail="Internal server error"
        )
```

### How to Integrate

1. Open `app/api/auth.py`
2. Find the existing `validate_token_with_auth_service` function
3. Replace the entire function with the code above
4. Ensure imports at top of file include: `os`, `logging`, `httpx`, `HTTPException`
5. Test: `AUTH_MODE=simple python -m pytest tests/test_auth.py`

---

## File 2: app/api/routes/superuser_routes.py

### Location
Find user registration endpoint in `app/api/routes/superuser_routes.py`

### What to Look For
```python
@router.post("/register-user")
def register_user_endpoint(user_data: ...):
    # ... existing implementation
```

### Code to Add at Top of File

```python
import os
import logging

logger = logging.getLogger(__name__)
AUTH_MODE = os.getenv("AUTH_MODE", "enterprise")
AUTH_SERVICE_URL = os.getenv("AUTH_SERVICE_URL", "http://nasiko-auth-service:8001")
```

### Replacement Endpoint Code

```python
@router.post("/register-user")
async def register_user_endpoint(user_data: dict):
    """
    Register new user in the system

    In AUTH_MODE=simple:
      - User registration skipped (single user only)
      - Returns success but no action taken

    In AUTH_MODE=enterprise:
      - Creates user via auth service
      - Full user management flow
    """

    # ==================== SIMPLE MODE (OSS) ====================
    if AUTH_MODE == "simple":
        logger.info("User registration skipped in simple auth mode (OSS)")
        return {
            "status": "skipped",
            "message": "User management disabled in single-tenant mode",
            "mode": "simple",
            "reason": "OSS version uses single default user"
        }

    # ==================== ENTERPRISE MODE ====================
    logger.info(f"Registering user in enterprise mode: {user_data.get('email')}")

    try:
        async with httpx.AsyncClient() as client:
            response = await client.post(
                f"{AUTH_SERVICE_URL}/auth/users/register",
                json=user_data,
                timeout=10
            )

            if response.status_code == 201:
                logger.info(f"User created successfully: {user_data.get('email')}")
                return response.json()

            elif response.status_code == 409:
                logger.warning(f"User already exists: {user_data.get('email')}")
                raise HTTPException(
                    status_code=409,
                    detail="User already exists"
                )

            else:
                logger.error(f"User registration failed: {response.status_code}")
                raise HTTPException(
                    status_code=response.status_code,
                    detail=f"User registration failed: {response.text}"
                )

    except httpx.ConnectError:
        logger.error(f"Cannot connect to auth service at {AUTH_SERVICE_URL}")
        raise HTTPException(
            status_code=503,
            detail="Auth service unavailable"
        )
    except Exception as e:
        logger.error(f"Error registering user: {e}")
        raise HTTPException(
            status_code=500,
            detail="Failed to register user"
        )


@router.get("/health")
async def superuser_health_check():
    """Health check endpoint for superuser service"""

    if AUTH_MODE == "simple":
        return {
            "status": "healthy",
            "mode": "simple",
            "auth_required": False,
            "user_registration_enabled": False
        }

    try:
        async with httpx.AsyncClient() as client:
            response = await client.get(
                f"{AUTH_SERVICE_URL}/health",
                timeout=5
            )
            auth_healthy = response.status_code == 200
    except:
        auth_healthy = False

    return {
        "status": "healthy" if auth_healthy else "degraded",
        "mode": "enterprise",
        "auth_service_healthy": auth_healthy,
        "user_registration_enabled": True
    }
```

### How to Integrate

1. Open `app/api/routes/superuser_routes.py`
2. Add imports at top: `os`, `logging` (if not already present)
3. Add `AUTH_MODE` and `AUTH_SERVICE_URL` constants
4. Find existing `register_user_endpoint` and replace with code above
5. Add health check endpoint
6. Test: `AUTH_MODE=simple python -m pytest tests/test_superuser.py`

---

## File 3: app/api/handlers/registry_handler.py

### Location
Find agent listing methods in `app/api/handlers/registry_handler.py`

### Code to Add at Top of Class/File

```python
import os
import logging

logger = logging.getLogger(__name__)
AUTH_MODE = os.getenv("AUTH_MODE", "enterprise")
AUTH_SERVICE_URL = os.getenv("AUTH_SERVICE_URL", "http://nasiko-auth-service:8001")
```

### Method 1: Agent Access Check

```python
async def can_user_access_agent(self, user_id: str, agent_id: str) -> bool:
    """
    Check if user has access to agent

    In simple mode: All agents accessible to single user
    In enterprise mode: Check via auth service ACL
    """

    if AUTH_MODE == "simple":
        logger.debug(f"Simple mode: user {user_id} has access to all agents")
        return True

    logger.debug(f"Enterprise mode: checking ACL for user {user_id} on agent {agent_id}")

    try:
        async with httpx.AsyncClient() as client:
            response = await client.get(
                f"{AUTH_SERVICE_URL}/auth/users/{user_id}/agents/{agent_id}/access",
                timeout=10
            )
            has_access = response.status_code == 200
            logger.debug(f"User {user_id} access to {agent_id}: {has_access}")
            return has_access
    except Exception as e:
        logger.warning(f"Failed to check ACL, denying access: {e}")
        return False
```

### Method 2: Get User Accessible Agents

```python
async def get_user_accessible_agents(self, user_id: str = "default"):
    """
    Get agents accessible to user

    In simple mode: Return all agents (single user has full access)
    In enterprise mode: Filter by ACL from auth service
    """

    if AUTH_MODE == "simple":
        logger.debug("Simple mode: returning all agents to single user")
        agents = await self.repo.get_all_agents()
        logger.debug(f"Returning {len(agents)} agents (all accessible)")
        return agents

    logger.debug(f"Enterprise mode: fetching agents for user {user_id}")

    try:
        # Get user's accessible agent IDs from auth service
        async with httpx.AsyncClient() as client:
            response = await client.get(
                f"{AUTH_SERVICE_URL}/auth/users/{user_id}/agents",
                timeout=10
            )

            if response.status_code == 200:
                agent_ids = response.json().get("agent_ids", [])
                logger.debug(f"User {user_id} has access to {len(agent_ids)} agents")

                if not agent_ids:
                    logger.debug("User has no accessible agents")
                    return []

                # Get agent details for allowed agent IDs
                agents = await self.repo.get_agents_by_ids(agent_ids)
                logger.debug(f"Returning {len(agents)} filtered agents")
                return agents
            else:
                logger.warning(f"Auth service returned {response.status_code}")
                return []

    except Exception as e:
        logger.error(f"Failed to get user accessible agents: {e}")
        return []
```

### Method 3: List Agents Endpoint

```python
async def list_agents(self, user_id: str = "default"):
    """
    List all agents for authenticated user

    Automatically handles AUTH_MODE:
    - simple: Returns all agents
    - enterprise: Returns user's accessible agents
    """

    if AUTH_MODE == "simple":
        logger.debug("Simple mode: listing all agents")
        agents = await self.repo.get_all_agents()
        logger.info(f"Listed {len(agents)} agents (simple mode)")
        return agents

    # Enterprise mode: Get user-specific agents
    agents = await self.get_user_accessible_agents(user_id)
    logger.info(f"Listed {len(agents)} agents for user {user_id} (enterprise mode)")
    return agents
```

### How to Integrate

1. Open `app/api/handlers/registry_handler.py`
2. Add imports: `os`, `logging`, `httpx` (if not present)
3. Add `AUTH_MODE` and `AUTH_SERVICE_URL` constants
4. Add/replace the three methods above
5. Update existing `list_agents()` to call `get_user_accessible_agents()`
6. Test: `AUTH_MODE=simple python -m pytest tests/test_registry.py`

---

## File 4: app/api/handlers/search_handler.py

### Location
Find user sync method in `app/api/handlers/search_handler.py`

### Code to Add at Top

```python
import os
import logging

logger = logging.getLogger(__name__)
AUTH_MODE = os.getenv("AUTH_MODE", "enterprise")
AUTH_SERVICE_URL = os.getenv("AUTH_SERVICE_URL", "http://nasiko-auth-service:8001")
```

### User Sync Method

```python
async def sync_users_from_auth_service(self):
    """
    Sync users for search indexing

    In simple mode: Skipped (single user only, no sync needed)
    In enterprise mode: Sync all users from auth service
    """

    if AUTH_MODE == "simple":
        logger.info("User sync skipped in simple auth mode (OSS single-tenant)")
        return {
            "status": "skipped",
            "mode": "simple",
            "message": "No user sync in single-tenant mode",
            "users_synced": 0
        }

    logger.info("Syncing users from auth service for enterprise mode")

    try:
        async with httpx.AsyncClient() as client:
            response = await client.get(
                f"{AUTH_SERVICE_URL}/auth/system/users-for-search",
                timeout=30
            )

            if response.status_code == 200:
                users = response.json().get("users", [])
                logger.info(f"Received {len(users)} users from auth service")

                # Index users for search
                indexed_count = 0
                for user in users:
                    try:
                        await self.index_user(user)
                        indexed_count += 1
                    except Exception as e:
                        logger.warning(f"Failed to index user: {e}")
                        continue

                logger.info(f"Successfully indexed {indexed_count}/{len(users)} users")
                return {
                    "status": "success",
                    "users_synced": indexed_count,
                    "users_total": len(users)
                }

            else:
                logger.warning(f"Auth service returned {response.status_code}")
                return {
                    "status": "failed",
                    "error": f"Auth service returned {response.status_code}",
                    "users_synced": 0
                }

    except httpx.ConnectError:
        logger.error(f"Cannot connect to auth service at {AUTH_SERVICE_URL}")
        return {
            "status": "failed",
            "error": "Auth service unavailable",
            "users_synced": 0
        }
    except Exception as e:
        logger.error(f"Failed to sync users: {e}")
        return {
            "status": "failed",
            "error": str(e),
            "users_synced": 0
        }
```

### How to Integrate

1. Open `app/api/handlers/search_handler.py`
2. Add imports: `os`, `logging`, `httpx` (if not present)
3. Add `AUTH_MODE` and `AUTH_SERVICE_URL` constants
4. Find existing `sync_users_from_auth_service` method
5. Replace with code above
6. Test: `AUTH_MODE=simple python -m pytest tests/test_search.py`

---

## Integration Checklist

After copying code snippets, follow this checklist:

### File 1: app/api/auth.py
- [ ] Replace `validate_token_with_auth_service` function
- [ ] Verify imports (os, logging, httpx)
- [ ] Test with `AUTH_MODE=simple` (no auth service needed)
- [ ] Test with `AUTH_MODE=enterprise` (calls auth service)

### File 2: app/api/routes/superuser_routes.py
- [ ] Add imports and constants
- [ ] Replace `register_user_endpoint` function
- [ ] Add `superuser_health_check` endpoint
- [ ] Test simple mode (registration skipped)
- [ ] Test enterprise mode (registration works)

### File 3: app/api/handlers/registry_handler.py
- [ ] Add imports and constants
- [ ] Add `can_user_access_agent` method
- [ ] Add `get_user_accessible_agents` method
- [ ] Update `list_agents` method
- [ ] Test simple mode (all agents visible)
- [ ] Test enterprise mode (filtered by ACL)

### File 4: app/api/handlers/search_handler.py
- [ ] Add imports and constants
- [ ] Replace `sync_users_from_auth_service` method
- [ ] Test simple mode (sync skipped)
- [ ] Test enterprise mode (sync works)

### Final Verification
- [ ] All 4 files updated
- [ ] No syntax errors
- [ ] Both modes tested
- [ ] Logging works correctly
- [ ] Error handling comprehensive

---

## Testing Commands

### Test Simple Mode (OSS)
```bash
# Set environment
export AUTH_MODE=simple

# Start backend
python -m app.main

# Test token validation (should work with any token)
curl -H "Authorization: Bearer any-token" \
  http://localhost:8000/api/v1/registries

# Test user registration (should be skipped)
curl -X POST http://localhost:8000/api/v1/superuser/register-user \
  -H "Content-Type: application/json" \
  -d '{"email": "test@test.com", "password": "password123"}'
```

### Test Enterprise Mode
```bash
# Set environment
export AUTH_MODE=enterprise
export AUTH_SERVICE_URL=http://localhost:8082

# Start backend
python -m app.main

# Get real token first
TOKEN=$(curl -X POST http://localhost:8082/auth/login \
  -d "username=admin&password=admin" | jq -r '.access_token')

# Test token validation (should call auth service)
curl -H "Authorization: Bearer $TOKEN" \
  http://localhost:8000/api/v1/registries

# Test user registration (should work)
curl -X POST http://localhost:8000/api/v1/superuser/register-user \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"email": "newuser@test.com", "password": "password123"}'
```

---

## Troubleshooting

### Issue: "AUTH_MODE not found"
**Solution**: Ensure environment variable is set
```bash
export AUTH_MODE=simple
python -m app.main
```

### Issue: "Auth service unavailable"
**Solution**: Check if running in enterprise mode without auth service
```bash
# Check current mode
echo $AUTH_MODE

# If enterprise but no auth service, either:
# 1. Change to simple mode: export AUTH_MODE=simple
# 2. Or start auth service: docker compose up nasiko-auth-service
```

### Issue: "Token validation always fails"
**Solution**: Verify correct auth service URL
```bash
# Test connectivity
curl http://localhost:8082/health

# Check environment
echo $AUTH_SERVICE_URL

# If wrong, update it
export AUTH_SERVICE_URL=http://localhost:8082
```

---

## Production Deployment

### OSS Deployment
```bash
# .env file
AUTH_MODE=simple

# No auth service needed
docker compose -f docker-compose.nasiko.yml up
```

### Enterprise Deployment
```bash
# .env file
AUTH_MODE=enterprise
AUTH_SERVICE_URL=http://nasiko-auth-service:8001

# Includes auth service
docker compose -f docker-compose.nasiko.yml up
```

---

**All code is production-ready and can be directly copied and integrated!**
