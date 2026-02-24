"""
Nasiko OSS Auth Service - Single-user authentication
Implements the same API contract as the enterprise auth service.

One default user is created on startup. GitHub OAuth maps to that user.
All agents are accessible (no ACL).
"""

import os
import uuid
import secrets
import logging
from datetime import datetime, timezone
from contextlib import asynccontextmanager
from typing import Optional

from fastapi import FastAPI, HTTPException, Request
from pydantic import BaseModel
from motor.motor_asyncio import AsyncIOMotorClient

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("nasiko-auth-oss")

# Configuration
MONGO_URL = os.getenv("MONGO_URL", "mongodb://admin:password@localhost:27017")
AUTH_DB_NAME = os.getenv("AUTH_DB_NAME", "nasiko")
PORT = int(os.getenv("PORT", "8001"))

# Database
db_client: Optional[AsyncIOMotorClient] = None
db = None

# The single default user - populated on startup
default_user: Optional[dict] = None


@asynccontextmanager
async def lifespan(app: FastAPI):
    global db_client, db, default_user
    db_client = AsyncIOMotorClient(MONGO_URL)
    db = db_client[AUTH_DB_NAME]

    # Ensure the single default user exists
    default_user = await db.oss_users.find_one({"username": "admin"})
    if not default_user:
        access_key = f"nak_{secrets.token_hex(16)}"
        access_secret = f"nas_{secrets.token_hex(32)}"
        now = datetime.now(timezone.utc).isoformat()
        default_user = {
            "user_id": str(uuid.uuid4()),
            "username": "admin",
            "email": "admin@localhost",
            "is_super_user": True,
            "access_key": access_key,
            "access_secret": access_secret,
            "is_active": True,
            "role": "Super User",
            "created_at": now,
            "updated_at": now,
        }
        await db.oss_users.insert_one(default_user)
        logger.info(f"Default user created: access_key={access_key}")
    else:
        logger.info(f"Default user exists: access_key={default_user['access_key']}")

    logger.info(f"OSS Auth Service ready (single-user mode, db={AUTH_DB_NAME})")
    yield
    db_client.close()


app = FastAPI(
    title="Nasiko OSS Auth Service",
    description="Single-user auth for Nasiko OSS",
    version="1.0.0",
    lifespan=lifespan,
)


# ============================================================================
# Models
# ============================================================================

class UserRegistrationRequest(BaseModel):
    username: str
    email: str
    is_super_user: bool = False


class UserCheckRequest(BaseModel):
    username: str


class GitHubAuthenticateRequest(BaseModel):
    github_id: str
    github_username: str
    email: str
    avatar_url: Optional[str] = None


class LoginRequest(BaseModel):
    access_key: str
    access_secret: str


# ============================================================================
# Helpers
# ============================================================================

def _user_response() -> dict:
    """Standard validation response for the single user"""
    return {
        "valid": True,
        "subject_id": default_user["user_id"],
        "username": default_user["username"],
        "email": default_user.get("email", ""),
        "subject_type": "user",
        "is_super_user": True,
        "organizations": ["default"],
        "oid": "default-org-id",
    }


def _extract_token(request: Request) -> str:
    """Extract token from Authorization header (Bearer or raw)"""
    auth_header = request.headers.get("authorization", "")
    if auth_header.startswith("Bearer "):
        return auth_header.split(" ", 1)[1]
    return auth_header


def _token_response() -> dict:
    """Standard token response for OSS login."""
    return {
        "token": default_user["access_key"],
        "token_type": "bearer",
        "expires_in": 3600,
        "is_super_user": True,
    }


# ============================================================================
# Endpoints
# ============================================================================

@app.get("/health")
async def health_check():
    try:
        await db.command("ping")
        return {"status": "healthy", "mode": "oss"}
    except Exception:
        return {"status": "degraded", "mode": "oss"}


# POST /auth/validate - Token validation
@app.post("/auth/validate")
async def validate_token(request: Request):
    """Any non-empty token is valid and maps to the single default user."""
    token = _extract_token(request)
    if not token:
        raise HTTPException(status_code=401, detail="Token required")
    return _user_response()


# POST /auth/users/register - User registration (no-op in OSS)
@app.post("/auth/users/register")
async def register_user(user_data: UserRegistrationRequest):
    """Returns the default user credentials. Only one user in OSS."""
    logger.info(f"Register request (OSS single-user): {user_data.username}")
    return {
        "user_id": default_user["user_id"],
        "username": default_user["username"],
        "email": default_user.get("email", ""),
        "is_super_user": True,
        "access_key": default_user["access_key"],
        "access_secret": default_user["access_secret"],
        "created_at": default_user.get("created_at", ""),
    }


# POST /auth/users/check - User existence check
@app.post("/auth/users/check")
async def check_user(data: UserCheckRequest):
    """The default admin user always exists."""
    return {"exists": True}


# POST /auth/users/login - Access key/secret login
@app.post("/auth/users/login")
async def login_user(data: LoginRequest):
    """Validate access key/secret and return a bearer token."""
    if data.access_key != default_user["access_key"] or data.access_secret != default_user["access_secret"]:
        raise HTTPException(status_code=401, detail="Invalid access key or secret")
    return _token_response()


# GET /auth/user - Current user
@app.get("/auth/user")
async def get_current_user(request: Request):
    """Return the current user for a valid token."""
    token = _extract_token(request)
    if not token:
        raise HTTPException(status_code=401, detail="Token required")
    if token != default_user["access_key"]:
        raise HTTPException(status_code=401, detail="Invalid token")

    now = datetime.now(timezone.utc).isoformat()
    return {
        "user_id": default_user["user_id"],
        "username": default_user["username"],
        "email": default_user.get("email", ""),
        "is_super_user": True,
        "is_active": True,
        "created_at": default_user.get("created_at", now),
        "last_login": now,
        "created_by": default_user["user_id"],
    }


# GET /auth/my-accessible-agents - All agents (no ACL)
@app.get("/auth/my-accessible-agents")
async def get_my_accessible_agents(request: Request):
    """Return all registered agents. No ACL in OSS."""
    all_agents = []
    try:
        async for reg in db.registries.find({}, {"id": 1, "_id": 0}):
            if reg.get("id"):
                all_agents.append(reg["id"])
    except Exception as e:
        logger.warning(f"Could not fetch agents: {e}")
    return {"accessible_agents": all_agents}


# GET /auth/agent-permissions/owner/{owner_id} - All agents with full access
@app.get("/auth/agent-permissions/owner/{owner_id}")
async def get_agents_by_owner(owner_id: str):
    """Return all agents with full permission. No ACL in OSS."""
    perms = []
    try:
        async for reg in db.registries.find({}, {"id": 1, "_id": 0}):
            if reg.get("id"):
                perms.append({"agent_id": reg["id"], "owner_id": owner_id, "permission_level": "full"})
    except Exception as e:
        logger.warning(f"Could not fetch agents: {e}")
    return {"agent_permissions": perms}


# POST /auth/agents/{agent_id}/permissions - No-op in OSS
@app.post("/auth/agents/{agent_id}/permissions")
async def create_agent_permissions(agent_id: str, owner_id: str = "default-user"):
    """No-op. All agents accessible to the single user."""
    return {"success": True, "message": "OSS mode - all agents accessible"}


# GET /auth/system/users-for-search - Single user for search indexing
@app.get("/auth/system/users-for-search")
async def get_users_for_search():
    """Return the single default user for search indexing."""
    return {
        "users": [{
            "id": default_user["user_id"],
            "username": default_user["username"],
            "display_name": default_user["username"],
            "email": default_user.get("email", ""),
            "role": "Super User",
            "avatar_url": None,
            "is_active": True,
            "created_at": default_user.get("created_at"),
            "updated_at": default_user.get("updated_at"),
        }]
    }


# POST /auth/github/authenticate - GitHub OAuth login
@app.post("/auth/github/authenticate")
async def github_authenticate(data: GitHubAuthenticateRequest):
    """
    GitHub OAuth callback from the backend.
    In OSS: always returns the single default user's token.
    Updates avatar/email if provided by GitHub.
    """
    # Update default user with GitHub info
    await db.oss_users.update_one(
        {"user_id": default_user["user_id"]},
        {"$set": {
            "github_id": data.github_id,
            "github_username": data.github_username,
            "avatar_url": data.avatar_url,
            "email": data.email,
            "updated_at": datetime.now(timezone.utc).isoformat(),
        }}
    )
    logger.info(f"GitHub login: {data.github_username} -> default user")

    return {
        "user_id": default_user["user_id"],
        "token": default_user["access_key"],
        "is_new_user": False,
        "is_super_user": True,
    }


if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=PORT)
