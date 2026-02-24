# AUTH_MODE Implementation Guide

## Overview

This document provides step-by-step instructions for implementing the `AUTH_MODE` feature flag to support both OSS (single-tenant) and Enterprise (multi-tenant) deployments without code duplication.

## Quick Summary

The AUTH_MODE feature flag allows the same codebase to operate in two modes:

- **`AUTH_MODE=simple`** (OSS): Single user, no auth validation
- **`AUTH_MODE=enterprise`** (Enterprise): Multi-tenant with full ACL

## Configuration

### 1. Environment Setup

Add `AUTH_MODE` to your environment configuration:

```bash
# .env or .env.local
AUTH_MODE=simple                    # For local OSS development

# Or for enterprise
AUTH_MODE=enterprise
AUTH_SERVICE_URL=http://localhost:8082
```

### 2. Docker Compose

Both environments are configured automatically:

```bash
# OSS (default)
AUTH_MODE=simple docker compose -f docker-compose.nasiko.yml up

# Enterprise
AUTH_MODE=enterprise docker compose -f docker-compose.nasiko.yml up
```

## Implementation Files (Production-Ready)

The following files need the AUTH_MODE implementation. Each is critical for production readiness.

### File 1: app/api/auth.py

**Purpose**: Token validation logic

**Status**: ⚠️ CRITICAL - Must be implemented

**What to do**:
Replace the token validation function with AUTH_MODE aware version:

```python
# Current implementation location: app/api/auth.py

import os
import logging
import httpx
from fastapi import HTTPException

logger = logging.getLogger(__name__)

AUTH_MODE = os.getenv("AUTH_MODE", "enterprise")
AUTH_SERVICE_URL = os.getenv("AUTH_SERVICE_URL", "http://nasiko-auth-service:8001")


async def validate_token_with_auth_service(token: str) -> dict:
    """
    Validate token - behavior depends on AUTH_MODE

    Args:
        token: JWT token to validate

    Returns:
        User info dict with user_id, username, etc.

    Raises:
        HTTPException: If token invalid
    """

    if AUTH_MODE == "simple":
        # OSS Mode: No validation, single user
        logger.debug("Using simple auth mode (OSS) - no token validation")
        if not token:
            raise HTTPException(status_code=401, detail="Token required")

        # Return default admin user
        return {
            "user_id": "default-user",
            "username": "admin",
            "email": "admin@localhost",
            "is_admin": True,
            "organizations": ["default"],
            "oid": "default-org-id"
        }

    # Enterprise Mode: Full auth service validation
    logger.debug(f"Using enterprise auth mode - validating with {AUTH_SERVICE_URL}")

    if not token:
        raise HTTPException(status_code=401, detail="Token required")

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
                logger.warning(f"Invalid token")
                raise HTTPException(status_code=401, detail="Invalid token")

            else:
                logger.error(f"Auth service error: {response.status_code}")
                raise HTTPException(
                    status_code=response.status_code,
                    detail="Auth service error"
                )

    except httpx.ConnectError:
        logger.error(f"Cannot connect to auth service: {AUTH_SERVICE_URL}")
        raise HTTPException(
            status_code=503,
            detail="Auth service unavailable"
        )
    except Exception as e:
        logger.error(f"Unexpected error during token validation: {e}")
        raise HTTPException(status_code=500, detail="Internal server error")
```

**How to integrate**:
1. Find the existing `validate_token_with_auth_service` function in `app/api/auth.py`
2. Replace it with the version above
3. Ensure imports are present (os, logging, httpx)
4. Test with both modes

---

### File 2: app/api/routes/superuser_routes.py

**Purpose**: User registration and management endpoints

**Status**: ⚠️ CRITICAL - Must be implemented

**What to do**:
Update the user registration endpoint to skip in simple mode:

```python
# In app/api/routes/superuser_routes.py

import os
import logging
from fastapi import APIRouter, HTTPException

logger = logging.getLogger(__name__)
AUTH_MODE = os.getenv("AUTH_MODE", "enterprise")
AUTH_SERVICE_URL = os.getenv("AUTH_SERVICE_URL", "http://nasiko-auth-service:8001")

router = APIRouter(prefix="/api/v1/superuser", tags=["superuser"])


@router.post("/register-user")
async def register_user_endpoint(user_data: dict):
    """
    Register new user in the system

    In simple mode: Skipped (single user only)
    In enterprise mode: Creates user via auth service
    """

    if AUTH_MODE == "simple":
        logger.info("User registration skipped in simple auth mode (OSS)")
        return {
            "status": "skipped",
            "message": "User management disabled in single-tenant mode",
            "mode": "simple"
        }

    # Enterprise: Call auth service
    logger.info(f"Registering user in enterprise mode: {user_data.get('email')}")

    try:
        async with httpx.AsyncClient() as client:
            response = await client.post(
                f"{AUTH_SERVICE_URL}/auth/users/register",
                json=user_data,
                timeout=10
            )

            if response.status_code == 201:
                return response.json()
            else:
                raise HTTPException(
                    status_code=response.status_code,
                    detail=f"User registration failed: {response.text}"
                )

    except Exception as e:
        logger.error(f"Failed to connect to auth service: {e}")
        raise HTTPException(
            status_code=503,
            detail="Auth service unavailable"
        )


@router.get("/health")
async def health_check():
    """Health check endpoint"""
    if AUTH_MODE == "simple":
        return {"status": "ok", "mode": "simple", "auth_required": False}

    # Check auth service health
    try:
        async with httpx.AsyncClient() as client:
            response = await client.get(
                f"{AUTH_SERVICE_URL}/health",
                timeout=5
            )
            return {
                "status": "ok" if response.status_code == 200 else "degraded",
                "mode": "enterprise",
                "auth_service": "available" if response.status_code == 200 else "unavailable"
            }
    except:
        return {
            "status": "degraded",
            "mode": "enterprise",
            "auth_service": "unavailable"
        }
```

**How to integrate**:
1. Find superuser routes in `app/api/routes/superuser_routes.py`
2. Add AUTH_MODE checks to user registration
3. Keep existing enterprise logic
4. Test both modes

---

### File 3: app/api/handlers/registry_handler.py

**Purpose**: Agent registry and ACL filtering

**Status**: ⚠️ CRITICAL - Must be implemented

**What to do**:
Update to return all agents in simple mode:

```python
# In app/api/handlers/registry_handler.py

import os
import logging

logger = logging.getLogger(__name__)
AUTH_MODE = os.getenv("AUTH_MODE", "enterprise")


class RegistryHandler:

    async def get_user_accessible_agents(self, user_id: str = "default"):
        """
        Get agents accessible to user

        In simple mode: All agents (single user has full access)
        In enterprise mode: Filter by ACL
        """

        if AUTH_MODE == "simple":
            logger.debug("Simple mode: returning all agents to single user")
            # Return all agents
            agents = await self.repo.get_all_agents()
            return agents

        # Enterprise: Filter by user's ACL
        logger.debug(f"Enterprise mode: filtering agents for user {user_id}")

        # Call auth service to get user's permissions
        async with httpx.AsyncClient() as client:
            response = await client.get(
                f"{AUTH_SERVICE_URL}/auth/users/{user_id}/agents",
                timeout=10
            )

            if response.status_code == 200:
                agent_ids = response.json().get("agent_ids", [])
                # Get agent details for allowed agent IDs
                agents = await self.repo.get_agents_by_ids(agent_ids)
                return agents
            else:
                logger.warning(f"Failed to get user agents from auth service")
                return []


    async def list_agents(self, user_id: str = "default"):
        """List agents for authenticated user"""

        if AUTH_MODE == "simple":
            logger.debug("Simple mode: listing all agents")
            return await self.repo.get_all_agents()

        # Enterprise mode
        return await self.get_user_accessible_agents(user_id)


    async def can_user_access_agent(self, user_id: str, agent_id: str) -> bool:
        """Check if user has access to agent"""

        if AUTH_MODE == "simple":
            # Single user has access to all agents
            logger.debug(f"Simple mode: user {user_id} can access all agents")
            return True

        # Enterprise: Check via auth service
        logger.debug(f"Enterprise mode: checking ACL for user {user_id} on agent {agent_id}")

        try:
            async with httpx.AsyncClient() as client:
                response = await client.get(
                    f"{AUTH_SERVICE_URL}/auth/users/{user_id}/agents/{agent_id}/access",
                    timeout=10
                )
                return response.status_code == 200
        except:
            logger.warning(f"Failed to check ACL")
            return False
```

**How to integrate**:
1. Find registry handler methods
2. Add AUTH_MODE checks before ACL filtering
3. Keep enterprise logic unchanged
4. Test both modes

---

### File 4: app/api/handlers/search_handler.py

**Purpose**: Search indexing and user sync

**Status**: ⚠️ CRITICAL - Must be implemented

**What to do**:
Skip user sync in simple mode:

```python
# In app/api/handlers/search_handler.py

import os
import logging

logger = logging.getLogger(__name__)
AUTH_MODE = os.getenv("AUTH_MODE", "enterprise")
AUTH_SERVICE_URL = os.getenv("AUTH_SERVICE_URL", "http://nasiko-auth-service:8001")


class SearchHandler:

    async def sync_users_from_auth_service(self):
        """
        Sync users for search indexing

        In simple mode: Skip (single user only)
        In enterprise mode: Sync from auth service
        """

        if AUTH_MODE == "simple":
            logger.info("User sync skipped in simple auth mode (OSS)")
            return {
                "status": "skipped",
                "mode": "simple",
                "message": "No user sync in single-tenant mode"
            }

        # Enterprise: Sync from auth service
        logger.info("Syncing users from auth service for enterprise mode")

        try:
            async with httpx.AsyncClient() as client:
                response = await client.get(
                    f"{AUTH_SERVICE_URL}/auth/system/users-for-search",
                    timeout=30
                )

                if response.status_code == 200:
                    users = response.json().get("users", [])
                    logger.info(f"Synced {len(users)} users")

                    # Index users for search
                    for user in users:
                        await self.index_user(user)

                    return {"status": "success", "users_synced": len(users)}
                else:
                    logger.warning(f"Auth service returned {response.status_code}")
                    return {"status": "failed", "error": "Auth service error"}

        except Exception as e:
            logger.error(f"Failed to sync users: {e}")
            return {"status": "failed", "error": str(e)}


    async def index_user(self, user: dict):
        """Index single user for search"""
        # Implementation depends on your search engine
        logger.debug(f"Indexing user: {user.get('email')}")
        # ... index logic ...
```

**How to integrate**:
1. Find search handler methods
2. Add AUTH_MODE checks to user sync
3. Keep enterprise logic unchanged
4. Test both modes

---

## Testing Checklist

### Simple Mode Testing (AUTH_MODE=simple)
```bash
# Start stack with simple mode
AUTH_MODE=simple docker compose -f docker-compose.nasiko.yml up

# Test 1: Backend should work WITHOUT auth service
curl http://localhost:8000/api/v1/healthcheck
# Expected: 200 OK

# Test 2: Any token should work
TOKEN="any-token-here"
curl -H "Authorization: Bearer $TOKEN" \
  http://localhost:8000/api/v1/registries
# Expected: 200 OK (returns all agents)

# Test 3: Missing token should fail
curl http://localhost:8000/api/v1/registries
# Expected: 401 Unauthorized

# Test 4: User registration should be skipped
curl -X POST http://localhost:8000/api/v1/superuser/register-user \
  -H "Content-Type: application/json" \
  -d '{"email": "test@test.com"}'
# Expected: 200 with "skipped" message

# Test 5: All agents visible to single user
curl -H "Authorization: Bearer test" \
  http://localhost:8000/api/v1/registries
# Expected: 200 with all agents listed
```

### Enterprise Mode Testing (AUTH_MODE=enterprise)
```bash
# Start stack with enterprise mode
AUTH_MODE=enterprise docker compose -f docker-compose.nasiko.yml up

# Test 1: Auth service should be running
curl http://localhost:8082/health
# Expected: 200 OK

# Test 2: Invalid token should fail
curl -H "Authorization: Bearer invalid-token" \
  http://localhost:8000/api/v1/registries
# Expected: 401 Unauthorized (from auth service)

# Test 3: Valid token should work
# (Use real token from auth service)
TOKEN=$(curl -X POST http://localhost:8082/auth/login \
  -d "username=admin&password=admin" | jq -r '.access_token')
curl -H "Authorization: Bearer $TOKEN" \
  http://localhost:8000/api/v1/registries
# Expected: 200 with user-specific agents

# Test 4: User registration should work
curl -X POST http://localhost:8000/api/v1/superuser/register-user \
  -H "Content-Type: application/json" \
  -d '{"email": "newuser@test.com", "password": "password123"}'
# Expected: 201 with user created

# Test 5: ACL should filter agents
# User should only see agents they have access to
curl -H "Authorization: Bearer $TOKEN" \
  http://localhost:8000/api/v1/registries
# Expected: 200 with filtered agents (ACL applied)
```

## Deployment Options

### Option 1: OSS Standalone
```bash
# No auth service needed
docker compose -f docker-compose.nasiko.yml up
# Automatically uses AUTH_MODE=simple
```

### Option 2: Enterprise with Auth Service
```bash
# Full enterprise setup
AUTH_MODE=enterprise docker compose -f docker-compose.nasiko.yml up
# Includes all services including auth-service
```

### Option 3: Custom Deployment
```bash
# Mix and match services
docker compose -f docker-compose.nasiko.yml up \
  -d kong mongodb redis nasiko-backend nasiko-web
# Manually deploy only needed services
```

## Production Checklist

- [ ] AUTH_MODE set correctly in environment
- [ ] All four files have AUTH_MODE implementation
- [ ] Simple mode tested without auth service
- [ ] Enterprise mode tested with auth service
- [ ] Token validation works in both modes
- [ ] ACL filtering works in enterprise mode
- [ ] User registration skipped in simple mode
- [ ] Error handling comprehensive
- [ ] Logging clear for debugging
- [ ] Documentation updated
- [ ] Backward compatibility verified

## Troubleshooting

### Issue: "Auth service unavailable"
```
Error in enterprise mode when auth service not running
Solution:
1. Ensure AUTH_MODE=simple if auth service not needed
2. Check AUTH_SERVICE_URL is correct
3. Verify auth service is healthy
```

### Issue: "Token validation failed"
```
Error in simple mode still validating tokens
Solution:
1. Check AUTH_MODE is actually set to "simple"
2. Verify environment variable loaded: echo $AUTH_MODE
3. Restart container after changing AUTH_MODE
```

### Issue: "User agents not filtered"
```
Enterprise mode returning all agents
Solution:
1. Verify AUTH_MODE=enterprise
2. Check auth service has ACL data for user
3. Verify auth service token endpoint working
```

## Support

For issues or questions:
1. Check testing checklist above
2. Review implementation files
3. Check logs for AUTH_MODE value
4. Verify environment configuration

---

**Status**: Production-Ready Implementation Guide
**Version**: 1.0
**Last Updated**: 2026-02-09
