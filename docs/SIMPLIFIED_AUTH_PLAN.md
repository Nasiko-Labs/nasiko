# Nasiko Auth Service - Simplified Architecture Plan

## Table of Contents
1. [System Overview](#system-overview)
2. [Architecture Components](#architecture-components)
3. [Database Schema Design](#database-schema-design)
4. [Kong Gateway Integration](#kong-gateway-integration)
5. [Permission Model](#permission-model)
6. [API Endpoints](#api-endpoints)
7. [Implementation Objectives](#implementation-objectives)
8. [Data Flow Diagrams](#data-flow-diagrams)

## System Overview

The Nasiko Auth Service provides centralized authentication and authorization for the entire Nasiko ecosystem, including user management and agent-to-agent (A2A) communication with a simplified permission model.

### Key Features:
- **User Authentication**: Secure user registration and token-based authentication
- **Super User Management**: Administrative control with environment-based setup
- **Agent Permissions**: Target-controlled access for agent-to-agent communication
- **Kong Integration**: Seamless API gateway authentication and authorization
- **Simplified Access Control**: Clear, non-conflicting permission management

## Architecture Components

```
┌─────────────────────────────────────────────────────────────────┐
│                    NASIKO ECOSYSTEM                            │
├─────────────────┬─────────────────┬─────────────────┬───────────┤
│   Client Apps   │   Kong Gateway  │  Registry API   │  Agents   │
└─────────────────┴─────────────────┴─────────────────┴───────────┘
                           │
                  ┌────────▼────────┐
                  │                 │
                  │  Auth Service   │
                  │                 │
                  └─────────────────┘
                           │
                  ┌────────▼────────┐
                  │                 │
                  │  Auth Database  │
                  │  (MongoDB)      │
                  │  + Redis Cache  │
                  └─────────────────┘
```

### Service Responsibilities:

| Component | Responsibility |
|-----------|----------------|
| **Auth Service** | User management, permissions, token generation/validation |
| **Registry Service** | Agent metadata, capabilities (caches user info) |
| **Kong Gateway** | API routing, rate limiting, calls auth service for validation |
| **Agents** | Business logic, authenticates via auth service |

## Database Schema Design

### Auth Service Database Collections

#### 1. Users Collection (Simplified)
```json
{
  "_id": "user_789",
  "username": "john_doe",
  "email": "john@company.com",
  "access_key": "NASK_xyz123",
  "access_secret_hash": "bcrypt_hash",
  "is_super_user": false,
  "is_active": true,
  "created_at": "2024-01-01T00:00:00Z",
  "created_by": "super_user_id"
}
```

#### 2. Agent Permissions Collection (Simplified)
```json
{
  "_id": "agent_perm_123",
  "agent_id": "agent_123",
  "owner_id": "user_789",

  // Only owner can manage (no delegation)
  "permission_managers": {
    "owner": "user_789"
  },

  // Target-controlled access only (who can call THIS agent)
  "access_permissions": {
    "can_be_accessed_by_users": ["user_101", "user_202"],
    "can_be_accessed_by_agents": ["agent_456", "agent_789"]
  },

  "created_at": "2024-01-01T00:00:00Z",
  "updated_at": "2024-01-01T00:00:00Z"
}
```

#### 3. Existing Collections (Enhanced)
```json
// entities collection (existing)
{
  "_id": "user_789",
  "type": "user",
  "name": "john_doe",
  "metadata": {"email": "john@company.com"},
  "created_at": "2024-01-01T00:00:00Z"
}

// user_credentials collection (existing)
{
  "user_id": "user_789",
  "access_key": "NASK_xyz123",
  "access_secret_hash": "bcrypt_hash",
  "is_super_user": false,
  "created_at": "2024-01-01T00:00:00Z",
  "last_used": null
}
```

## Kong Gateway Integration

### Authentication & Authorization Flow

```
┌─────────┐    ┌──────────┐    ┌─────────────┐    ┌─────────┐
│ Client  │───▶│   Kong   │───▶│Auth Service │───▶│ Agent   │
│         │    │ Gateway  │    │             │    │   API   │
└─────────┘    └──────────┘    └─────────────┘    └─────────┘
     │              │                   │              │
     │              │                   │              │
     ▼              ▼                   ▼              ▼
1. Send JWT    2. Validate       3. Check         4. Forward
   Token          Token             Permissions      Request
```

### Kong Plugin Configuration

1. **JWT Plugin**: Validates JWT tokens from auth service
2. **Custom Auth Plugin**: Calls auth service for permission validation

#### Kong Routes with Auth
```lua
-- Kong validates JWT and calls auth service
POST /auth/validate     -- Token validation
POST /auth/authorize   -- Permission checking

-- Example Kong route configuration
{
  "paths": ["/api/agents/agent-123/*"],
  "plugins": [
    {
      "name": "jwt",
      "config": {
        "secret_is_base64": false,
        "key_claim_name": "token_id"
      }
    },
    {
      "name": "auth-validator",
      "config": {
        "auth_service_url": "http://auth-service:8001"
      }
    }
  ]
}
```

## Permission Model

### Simplified Hierarchy
```
Super User (Global Admin)
    │
    └── Users (Agent Owners)
        └── Agents (Owned by users)
```

### Permission Types

#### User Permissions
- **Super User Status**: Global admin privileges
- **Active/Inactive Status**: User account status

#### Agent Permissions (Target-Controlled)
- **can_be_accessed_by_users**: Individual users that can use this agent
- **can_be_accessed_by_agents**: Other agents that can communicate with this agent

### Permission Management Rights

| Role | Can Manage |
|------|------------|
| **Super User** | All users, all agents, all permissions |
| **Agent Owner** | Own agents' permissions only |

### Permission Logic (Simplified)

#### Agent-to-Agent Access:
```python
async def can_agent_access_agent(caller_agent_id: str, target_agent_id: str) -> bool:
    """
    Target agent controls who can access it
    """
    target_permissions = await get_agent_permissions(target_agent_id)

    if not target_permissions:
        return True  # No permissions record = allow all

    allowed_agents = target_permissions.get("access_permissions", {}).get("can_be_accessed_by_agents", [])

    # If no restrictions defined, allow all agents
    if not allowed_agents:
        return True

    # If restrictions exist, caller must be in the list
    return caller_agent_id in allowed_agents
```

#### User-to-Agent Access:
```python
async def can_user_access_agent(user_id: str, agent_id: str) -> bool:
    """
    Check if user can access agent
    """
    # Super users can access everything
    user = await get_user(user_id)
    if user.get("is_super_user"):
        return True

    # Check agent's user access list
    agent_permissions = await get_agent_permissions(agent_id)
    if not agent_permissions:
        return False  # No permissions = no access for users

    allowed_users = agent_permissions.get("access_permissions", {}).get("can_be_accessed_by_users", [])
    return user_id in allowed_users
```

## API Endpoints

### Authentication Endpoints
```http
POST /auth/users/register              # Super user creates users
POST /auth/users/login                 # User authentication
POST /auth/agents/token                # Agent authentication
POST /auth/validate                    # Token validation (Kong)
POST /auth/authorize                   # Permission check (Kong)
```

### User Management
```http
GET    /auth/users/{user_id}           # Get user details
PUT    /auth/users/{user_id}           # Update user (super user only)
POST   /auth/users/{user_id}/regenerate-credentials
DELETE /auth/users/{user_id}           # Deactivate user
GET    /auth/users                     # List users (with filters)
```

### Agent Permission Management (Simplified)
```http
# Grant user access TO this agent
POST   /auth/agents/{agent_id}/access/users
Body: {"user_ids": ["user_101", "user_202"]}

# Grant agent access TO this agent
POST   /auth/agents/{agent_id}/access/agents
Body: {"agent_ids": ["agent_456", "agent_789"]}

# Revoke access
DELETE /auth/agents/{agent_id}/access/users/{user_id}
DELETE /auth/agents/{agent_id}/access/agents/{agent_id}

# Get current permissions
GET    /auth/agents/{agent_id}/permissions
```

### Token Management
```http
DELETE /auth/tokens/revoke             # Revoke current token
POST   /auth/tokens/revoke-user/{user_id}         # Revoke user's tokens
POST   /auth/emergency/revoke-all      # Emergency: revoke all tokens
```

## Implementation Objectives

### ✅ Objective 1: User Registration with Permissions
**Implementation:**
- Super user creates user accounts via `/auth/users/register`
- Auto-generates unique access_key + access_secret
- Sets user as active by default
- No organizational assignments in MVP

**Flow:**
```
Super User → Create User → Auto-generate Credentials → Store User
```

### ✅ Objective 2: Agent Registration with Permissions
**Implementation:**
- When agent deployed via orchestrator/nasiko-backend
- Auto-creates entry in `agent_permissions` collection
- Sets deploying user as owner
- Initial permissions: empty (owner must explicitly grant access)

**Flow:**
```
User Deploys Agent → Orchestrator → Registry Service → Auth Service (create permissions)
```

### ✅ Objective 3: Change User Permissions (Super User Only)
**Implementation:**
- Super user can modify any user's:
  - Active/inactive status
  - Super user status
  - Basic profile information

### ✅ Objective 4: User Addition/Invite by SuperUser
**Implementation:**
- Super user creates account via API
- Returns access_key + access_secret (one-time visibility)
- User cannot see secret again (only regenerate)
- Super user can regenerate credentials anytime

### ✅ Objective 5: Change Agent Permissions (Simplified)
**Implementation:**
- Only two levels of control:
  - **Agent Owner**: Full control over their agents
  - **Super User**: Global control over all agents

### ✅ Objective 6: Permission Types (Simplified)
**User Permissions:**
- Super user privileges
- Active/inactive status

**Agent Permissions (Target-Controlled):**
- User-based access control
- Agent-to-agent communication (directional, target-controlled)

### ✅ Objective 7: User Token Generation
**Implementation:**
- User provides access_key + access_secret
- Returns JWT containing user context
- Token includes user_id and super_user status

### ✅ Objective 8: Agent Token Generation
**Implementation:**
- Agent provides agent_id
- Returns JWT with agent identity
- Token contains agent_id for permission checks

### ✅ Objective 9: Super User Generation
**Implementation:**
- Environment-variable based initial setup
- Check if any super user exists on startup
- If none exist, create from env vars
- Subsequent super users created by existing super users

## Data Flow Diagrams

### 1. User Authentication Flow
```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│    User     │───▶│Auth Service │───▶│   Database  │
│             │    │             │    │             │
└─────────────┘    └─────────────┘    └─────────────┘
       │                    │                  │
       │ 1. access_key +    │ 2. Verify       │
       │    access_secret   │    credentials  │
       │                    │                 │
       │ 4. JWT Token      │ 3. Generate     │
       │ <─────────────────│    JWT          │
       │                    │ <───────────────│
```

### 2. Agent-to-Agent Communication Flow (Simplified)
```
┌─────────┐   ┌──────────┐   ┌─────────────┐   ┌─────────┐
│Agent X  │──▶│   Kong   │──▶│Auth Service │──▶│Agent Y  │
│         │   │ Gateway  │   │             │   │         │
└─────────┘   └──────────┘   └─────────────┘   └─────────┘
     │             │               │               │
     │ 1. JWT      │ 2. Validate   │ 3. Check:     │ 4. Forward
     │    Token    │    Token      │    Can X      │    Request
     │             │               │    access Y?  │
```

### 3. Permission Management Flow (Simplified)
```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│Agent Owner  │───▶│Auth Service │───▶│   Database  │
│             │    │             │    │             │
└─────────────┘    └─────────────┘    └─────────────┘
       │                    │                  │
       │ 1. Grant user      │ 2. Validate      │
       │    access to       │    ownership     │
       │    agent           │                  │
       │                    │ 3. Update        │
       │ 4. Success         │    permissions   │
       │ <─────────────────│ <────────────────│
```

### 4. Kong Gateway Integration Flow
```
┌─────────┐   ┌──────────┐   ┌─────────────┐   ┌─────────────┐
│ Client  │──▶│   Kong   │──▶│Auth Service │──▶│Target Agent │
│         │   │ Gateway  │   │             │   │     API     │
└─────────┘   └──────────┘   └─────────────┘   └─────────────┘
     │             │               │                   │
     │ 1. Request  │ 2. Extract    │ 3. Validate       │ 5. Forward
     │    with JWT │    JWT        │    + Authorize    │    Request
     │             │               │                   │
     │ 6. Response │ 4. Allow/Deny │                   │
     │ <───────────│ <─────────────│                   │
```

## Key Simplifications Made

### ❌ **Removed from Original Plan:**
- Departments and teams collections
- Organizational hierarchy and roles
- Permission delegation system
- Team/department-based access control
- Complex role-based permissions
- Multi-level management rights

### ✅ **Kept Essential Features:**
- Super user and regular user management
- Agent ownership and permissions
- Target-controlled access model
- Kong gateway integration
- JWT token authentication
- Basic user and agent CRUD operations

### 🔄 **Permission Model Changes:**
- **Before**: Complex bidirectional permissions with potential conflicts
- **After**: Simple target-controlled access - each agent controls who can access it
- **Before**: Teams/departments could access agents
- **After**: Only individual users can access agents (plus A2A)
- **Before**: Multiple management roles (owner, delegate, team lead, etc.)
- **After**: Only owner and super user can manage agent permissions

## Benefits of Simplified Model

1. **No Contradictions**: Only target agent controls access, no conflicts possible
2. **Clear Logic**: Easy to understand "who can call what"
3. **Default Open A2A**: Agents can communicate unless explicitly blocked
4. **Simple Implementation**: Fewer collections, simpler logic
5. **Future Extensible**: Can add teams/departments later without breaking existing functionality
6. **Kong Ready**: Clean validate/authorize endpoints for gateway integration

---

This simplified architecture provides a solid, conflict-free foundation for authentication and authorization that can be extended with additional organizational features in future iterations.