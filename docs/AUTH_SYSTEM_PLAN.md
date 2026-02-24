# Nasiko Auth Service - Complete Architecture Plan

## Table of Contents
1. [System Overview](#system-overview)
2. [Architecture Components](#architecture-components)
3. [Database Schema Design](#database-schema-design)
4. [Kong Gateway Integration](#kong-gateway-integration)
5. [Permission Model](#permission-model)
6. [API Endpoints](#api-endpoints)
7. [Implementation Objectives](#implementation-objectives)
8. [Data Flow Diagrams](#data-flow-diagrams)
9. [Implementation Phases](#implementation-phases)

## System Overview

The Nasiko Auth Service provides centralized authentication and authorization for the entire Nasiko ecosystem, including user management, agent-to-agent (A2A) communication, and organizational hierarchy management.

### Key Features:
- **User Authentication**: Secure user registration and token-based authentication
- **Organizational Structure**: Departments → Teams → Users hierarchy
- **Agent Permissions**: Granular access control for agent-to-agent communication
- **Kong Integration**: Seamless API gateway authentication and authorization
- **Permission Delegation**: Flexible permission management with role-based delegation

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

#### 1. Users Collection
```json
{
  "_id": "user_789",
  "username": "john_doe",
  "email": "john@company.com",
  "access_key": "NASK_xyz123",
  "access_secret_hash": "bcrypt_hash",
  "is_super_user": false,
  "department_id": "dept_123",
  "team_id": "team_456",
  "team_role": "lead",          // "lead" | "member"
  "department_role": "member",   // "manager" | "member"
  "is_active": true,
  "metadata": {},
  "created_at": "2024-01-01T00:00:00Z",
  "created_by": "super_user_id"
}
```

#### 2. Departments Collection
```json
{
  "_id": "dept_123",
  "name": "Engineering",
  "description": "Engineering Department",
  "manager_id": "user_456",
  "is_active": true,
  "created_by": "super_user_id",
  "created_at": "2024-01-01T00:00:00Z"
}
```

#### 3. Teams Collection
```json
{
  "_id": "team_456",
  "name": "AI Team",
  "department_id": "dept_123",
  "lead_id": "user_789",
  "members": ["user_789", "user_101"],
  "is_active": true,
  "created_by": "super_user_id",
  "created_at": "2024-01-01T00:00:00Z"
}
```

#### 4. Agent Permissions Collection
```json
{
  "_id": "agent_perm_123",
  "agent_id": "agent_123",
  "owner_id": "user_789",

  // Permission Management Rights
  "permission_managers": {
    "owner": "user_789",
    "delegates": ["user_101"],
    "team_lead_access": true,
    "dept_manager_access": false
  },

  // Access Control Lists
  "access_permissions": {
    "can_be_accessed_by_teams": ["team_456"],
    "can_be_accessed_by_users": ["user_101"],
    "can_access_agents": ["agent_456", "agent_789"]
  },

  "created_at": "2024-01-01T00:00:00Z",
  "updated_at": "2024-01-01T00:00:00Z"
}
```

#### 5. Existing Collections (Enhanced)
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

### Organizational Hierarchy
```
Super User (Global Admin)
    │
    ├── Department Manager
    │   └── Department (Engineering, Marketing, etc.)
    │       │
    │       ├── Team Lead
    │       │   └── Team (AI Team, Backend Team, etc.)
    │       │       │
    │       │       └── Team Members (Users)
    │       │
    │       └── Agent Owners (Users who deployed agents)
    │           └── Agents (Owned by users)
```

### Permission Types

#### User Permissions
- **Department Assignment**: Which department user belongs to
- **Team Assignment**: Which team within department
- **Role Assignment**: lead, member, manager
- **Super User Status**: Global admin privileges

#### Agent Permissions (Directional)
- **can_be_accessed_by_teams**: Teams that can use this agent
- **can_be_accessed_by_users**: Individual users that can use this agent
- **can_access_agents**: Other agents this agent can communicate with (A2A)

### Permission Management Rights

| Role | Can Manage |
|------|------------|
| **Super User** | All users, all agents, all permissions |
| **Agent Owner** | Own agents' permissions |
| **Delegates** | Agents where explicitly granted management rights |
| **Team Lead** | Agents owned by team members (if enabled by owner) |
| **Dept Manager** | Agents owned by department members (if enabled by owner) |

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

### Organization Management
```http
POST   /auth/departments               # Create department
PUT    /auth/departments/{dept_id}     # Update department
GET    /auth/departments               # List departments
POST   /auth/teams                     # Create team
PUT    /auth/teams/{team_id}           # Update team
GET    /auth/teams                     # List teams
PUT    /auth/users/{user_id}/assignment # Change user's dept/team
```

### Agent Permission Management
```http
POST   /auth/agents/{agent_id}/delegates          # Delegate management
POST   /auth/agents/{agent_id}/access/teams       # Grant team access
POST   /auth/agents/{agent_id}/access/users       # Grant user access
POST   /auth/agents/{agent_id}/access/agents      # Grant A2A access
DELETE /auth/agents/{agent_id}/access/teams/{team_id}
DELETE /auth/agents/{agent_id}/access/users/{user_id}
DELETE /auth/agents/{agent_id}/access/agents/{agent_id}
GET    /auth/agents/{agent_id}/permissions        # Get agent permissions
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
- Assigns user to department/team during creation
- Sets initial role (member by default, can be changed)

**Flow:**
```
Super User → Create User → Auto-generate Credentials → Assign to Org Structure
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
  - Department/team assignments
  - Roles within org structure
  - Active/inactive status
  - Super user status

### ✅ Objective 4: User Addition/Invite by SuperUser
**Implementation:**
- Super user creates account via API
- Returns access_key + access_secret (one-time visibility)
- User cannot see secret again (only regenerate)
- Super user can regenerate credentials anytime

### ✅ Objective 5: Change Agent Permissions
**Implementation:**
- Multi-level permission management:
  - **Agent Owner**: Full control
  - **Delegates**: Granted by owner
  - **Team Lead**: If enabled by owner
  - **Dept Manager**: If enabled by owner
  - **Super User**: Global control

### ✅ Objective 6: Permission Types
**User Permissions:**
- Organizational placement (dept/team)
- Role-based access (lead/member/manager)
- Super user privileges

**Agent Permissions:**
- Team-based access control
- User-based access control
- Agent-to-agent communication (directional)

### ✅ Objective 7: User Token Generation
**Implementation:**
- User provides access_key + access_secret
- Returns JWT containing user context and permissions
- Token includes department/team/role information

### ✅ Objective 8: Agent Token Generation
**Implementation:**
- Agent provides agent_id
- Returns JWT with agent's A2A permissions
- Token contains list of agents this agent can access

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

### 2. Agent-to-Agent Communication Flow
```
┌─────────┐   ┌──────────┐   ┌─────────────┐   ┌─────────┐
│Agent X  │──▶│   Kong   │──▶│Auth Service │──▶│Agent Y  │
│         │   │ Gateway  │   │             │   │         │
└─────────┘   └──────────┘   └─────────────┘   └─────────┘
     │             │               │               │
     │ 1. JWT      │ 2. Validate   │ 3. Check      │ 4. Forward
     │    Token    │    Token      │    A2A Perms  │    Request
```

### 3. Permission Management Flow
```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│Agent Owner  │───▶│Auth Service │───▶│   Database  │
│             │    │             │    │             │
└─────────────┘    └─────────────┘    └─────────────┘
       │                    │                  │
       │ 1. Grant team      │ 2. Validate      │
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

## Implementation Phases

### Phase 1: Foundation (Weeks 1-2)
**Deliverables:**
- [ ] Super user creation mechanism
- [ ] Basic user registration/authentication
- [ ] User token generation and validation
- [ ] Kong integration for token validation

**APIs:**
- `POST /auth/users/register`
- `POST /auth/users/login`
- `POST /auth/validate`

### Phase 2: Organization Structure (Weeks 3-4)
**Deliverables:**
- [ ] Department and team collections
- [ ] User assignment to organizational units
- [ ] Role-based permissions within teams/departments
- [ ] Enhanced user management APIs

**APIs:**
- `POST /auth/departments`
- `POST /auth/teams`
- `PUT /auth/users/{id}/assignment`

### Phase 3: Agent Permissions (Weeks 5-6)
**Deliverables:**
- [ ] Agent permissions collection
- [ ] Basic agent access control
- [ ] Agent token generation
- [ ] Kong integration for agent authorization

**APIs:**
- `POST /auth/agents/token`
- `POST /auth/authorize`
- `POST /auth/agents/{id}/access/*`

### Phase 4: Advanced Features (Weeks 7-8)
**Deliverables:**
- [ ] Permission delegation system
- [ ] A2A communication permissions
- [ ] Audit trails and logging
- [ ] Advanced user management features

**APIs:**
- `POST /auth/agents/{id}/delegates`
- Advanced permission management endpoints

### Phase 5: Production Hardening (Weeks 9-10)
**Deliverables:**
- [ ] Performance optimization
- [ ] Security hardening
- [ ] Monitoring and alerting
- [ ] Documentation completion

---

## Notes

1. **Token Expiration**: All JWT tokens expire after 12 hours
2. **Redis Caching**: Agent and user permissions cached for performance
3. **Audit Trails**: All permission changes logged for compliance
4. **Security**: All passwords hashed with bcrypt, secrets generated securely
5. **Scalability**: Stateless design allows horizontal scaling

This architecture provides a robust, scalable authentication and authorization system that supports the complex organizational and agent interaction patterns required by the Nasiko ecosystem.