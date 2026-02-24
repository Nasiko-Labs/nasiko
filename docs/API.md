# Nasiko API Documentation

## Overview

FastAPI-based REST API for managing AI agents, chat tracking, agent uploads, GitHub integration, and observability. The API uses a modular repository architecture for efficient data management and supports comprehensive agent lifecycle management.

**Base URL:** `http://localhost:8000/api/v1`
**Interactive Documentation:** `http://localhost:8000/docs`
**Alternative Docs:** `http://localhost:8000/redoc`

## Response Format

**Success:**
```json
{
  "data": {...},
  "status_code": 200,
  "message": "Success message"
}
```

**Error:**
```json
{
  "error": "Error description",
  "status_code": 400,
  "message": "User-friendly message"
}
```

## Health Check

### Health Check
`GET /healthcheck`

Simple health check endpoint.

```json
{
  "status": "ok"
}
```

## Agent Registry Endpoints

### Create Agent Registry
`POST /registry`

```json
{
  "agent": {
    "id": "document-expert-v1",
    "name": "document-expert",
    "version": "1.0.0",
    "tags": ["document", "pdf", "analysis"],
    "description": "Document processing and Q&A"
  },
  "capabilities": [
    {
      "id": "chat",
      "name": "Chat with Document Expert",
      "description": "Process documents and answer questions",
      "endpoint": {
        "path": "/chat",
        "method": "POST",
        "headers": {}
      },
      "input_schema": {
        "session_id": "string",
        "message": "string",
        "files": "file (optional)"
      },
      "output_schema": {
        "response": "string",
        "confidence": "number"
      },
      "tags": ["chat", "document"]
    }
  ],
  "health_check": {
    "endpoint": "/health",
    "method": "GET",
    "expected_status": 200
  },
  "agent_url": "http://localhost:8001",
  "metadata": {
    "created_by": "system",
    "model_used": "gpt-4"
  }
}
```

### Get My Agents (Simple)
`GET /registry/user/agents`

**Headers**: `Authorization: Bearer <token>`

Get all agents available to the authenticated user (both uploaded and accessible from registry) in simple format.

### Get My Agents (Detailed)
`GET /registry/user/agents/info`

**Headers**: `Authorization: Bearer <token>`

Get detailed information of all agents available to the authenticated user.

### Get Agent by Name
`GET /registry/agent/name/{agent_name}`

Get agent details using the agent name.

### Get Agent by ID
`GET /registry/agent/id/{agent_id}`

**Headers**: `Authorization: Bearer <token>`

Get agent details using the agent ID from the AgentCard (requires authentication).

### Upsert Agent Registry
`PUT /registry/agent/{agent_name}`

Create or update a registry entry by agent name.

## Chat Session Management Endpoints

### Create Chat Session
`POST /chat/session`

**Headers**: `Authorization: Bearer <token>`

**Request Body**:
```json
{
  "agent_name": "document-expert"  // Optional
}
```

**Response**:
```json
{
  "session_id": "generated-uuid",
  "created_at": "2024-01-15T10:30:00Z",
  "title": "Quick Chat"
}
```

### Get Session History
`GET /chat/session/list`

**Headers**: `Authorization: Bearer <token>`

**Query Parameters**:
- `limit`: Number of sessions per page (1-20, default: 10)
- `cursor`: Pagination cursor
- `direction`: "before" or "after" (default: "after")

**Response**:
```json
{
  "messages": [
    {
      "session_id": "session-uuid",
      "created_at": "2024-01-15T10:30:00Z",
      "title": "Quick Chat",
      "agent_name": "document-expert"
    }
  ],
  "total_count": 50,
  "has_more": true,
  "next_cursor": "2024-01-15T10:30:00Z",
  "prev_cursor": null
}
```

### Delete Chat Session
`DELETE /chat/session/{session_id}`

**Headers**: `Authorization: Bearer <token>`

### Get Chat History for Session
`GET /chat/session/{session_id}`

**Headers**: `Authorization: Bearer <token>`

**Query Parameters**:
- `limit`: Number of messages per page (1-100, default: 50)
- `cursor`: Pagination cursor
- `direction`: "before" or "after" (default: "after")

**Response**:
```json
{
  "messages": [
    {
      "session_id": "session-uuid",
      "timestamp": "2024-01-15T10:30:00Z",
      "content": "Hello, how can I help?",
      "role": "assistant"
    }
  ],
  "total_count": 50,
  "has_more": true,
  "next_cursor": "2024-01-15T10:30:00Z",
  "prev_cursor": null
}

## Agent Upload Endpoints

### Upload Agent ZIP File
`POST /agents/upload`

```bash
curl -X POST http://localhost:8000/api/v1/agents/upload \
  -F "file=@agent.zip" \
  -F "agent_name=my-agent"
```

**Required files in ZIP:**
- `Dockerfile`
- `docker-compose.yml`
- `src/main.py` OR `main.py`

**Optional files:**
- `capabilities.json` (auto-generated if missing)

### Upload Agent Directory
`POST /agents/upload-directory`

**Headers**: `Authorization: Bearer <token>`

```json
{
  "directory_path": "/path/to/agent",
  "agent_name": "my-agent"
}
```

### Upload Process Flow
1. Validate agent structure
2. Generate capabilities.json if missing
3. Trigger orchestration
4. Return upload status with tracking ID

## GitHub Integration Endpoints

**📖 For easier GitHub operations, use the [Nasiko CLI](CLI.md) which provides organized command groups.**

### GitHub OAuth Login
`GET /auth/github/login`

**Headers**: `Authorization: Bearer <token>`

**Status Code**: 307 (Temporary Redirect)

Redirects the user to GitHub for authentication.

### GitHub OAuth Callback
`GET /auth/github/callback`

**Query Parameters**:
- `code`: OAuth code from GitHub
- `state`: User ID used as state parameter

**Response**: HTML page showing success/failure

### Get GitHub Token Status
`GET /auth/github/token`

**Headers**: `Authorization: Bearer <token>`

Get current user's GitHub access token status.

### GitHub Logout
`POST /auth/github/logout`

**Headers**: `Authorization: Bearer <token>`

Logout from GitHub - removes stored token for user.

### List GitHub Repositories
`GET /github/repositories`

**Headers**: `Authorization: Bearer <token>`

**Response**:
```json
{
  "data": {
    "repositories": [
      {
        "name": "my-agent",
        "full_name": "user/my-agent",
        "description": "My AI agent",
        "clone_url": "https://github.com/user/my-agent.git",
        "html_url": "https://github.com/user/my-agent",
        "private": false,
        "language": "Python"
      }
    ]
  }
}
```

### Clone GitHub Repository
`POST /github/clone`

**Headers**: `Authorization: Bearer <token>`

**Status Code**: 201

```json
{
  "repository_url": "https://github.com/user/repo",
  "agent_name": "github-agent",
  "branch": "main"
}
```

## N8N Integration Endpoints

### Register N8N Workflow as Agent
`POST /agents/n8n/register`

**Headers**: `Authorization: Bearer <token>`

Register an N8N workflow as an A2A agent with user ownership.

### Save N8N Credentials
`POST /agents/n8n/connect`

**Headers**: `Authorization: Bearer <token>`

**Status Code**: 201

Test N8N connection first, then save credentials only if test succeeds.

**Request Body**:
```json
{
  "connection_name": "My N8N Instance",
  "n8n_url": "https://my-n8n.example.com",
  "api_key": "your-api-key"
}
```

**Response**:
```json
{
  "success": true,
  "message": "N8N credentials saved successfully",
  "connection_name": "My N8N Instance",
  "test_status": "success"
}
```

### Get N8N Credentials
`GET /agents/n8n/credentials`

**Headers**: `Authorization: Bearer <token>`

Get authenticated user's saved N8N credentials (without sensitive data).

**Response**:
```json
{
  "connection_name": "My N8N Instance",
  "n8n_url": "https://my-n8n.example.com",
  "connection_status": "active",
  "last_tested": "2024-01-15T10:30:00Z",
  "created_at": "2024-01-10T10:30:00Z",
  "updated_at": "2024-01-15T10:30:00Z"
}
```

### Update N8N Credentials
`PUT /agents/n8n/credentials`

**Headers**: `Authorization: Bearer <token>`

Update authenticated user's N8N credentials.

**Request Body**:
```json
{
  "n8n_url": "https://new-n8n.example.com",
  "api_key": "new-api-key",
  "is_active": true
}
```

### Delete N8N Credentials
`DELETE /agents/n8n/credentials`

**Headers**: `Authorization: Bearer <token>`

Delete authenticated user's N8N credentials permanently.

### List N8N Workflows
`GET /agents/n8n/workflows`

**Headers**: `Authorization: Bearer <token>`

**Query Parameters**:
- `active_only`: Filter to show only active workflows (default: true)
- `limit`: Maximum number of workflows to return (default: 100)

**Response**:
```json
{
  "workflows": [
    {
      "id": "workflow-123",
      "name": "Chat Workflow",
      "active": true,
      "is_chat_workflow": true,
      "node_count": 5,
      "updated_at": "2024-01-15T10:30:00Z",
      "tags": ["chat", "ai"]
    }
  ],
  "total_count": 10
}
```

## Search Endpoints

### Search Users
`GET /search/users`

**Headers**: `Authorization: Bearer <token>`

**Query Parameters**:
- `q`: Search query (minimum 2 characters)
- `limit`: Maximum number of results (max 50, default: 10)

**Features**:
- Prefix matching, case insensitive, fuzzy matching
- Searches username, display_name, and email
- Results ranked by relevance

### Search Agents
`GET /search/agents`

**Headers**: `Authorization: Bearer <token>`

**Query Parameters**:
- `q`: Search query (minimum 2 characters)
- `limit`: Maximum number of results (max 50, default: 10)

**Features**:
- Prefix matching, case insensitive, fuzzy matching
- Tag-based search and description search
- Results ranked by relevance

### Index User (Internal)
`POST /search/index/user`

**Description**: Internal system endpoint called by auth service when new users are registered.

## Super User Management Endpoints

### Register New User
`POST /user/register`

**Headers**: `Authorization: Bearer <super-user-token>`

**Description**: Register a new user in the system (Super User Only)

**Request Body**:
```json
{
  "username": "new_user",
  "email": "user@example.com",
  "is_super_user": false
}
```

**Response**:
```json
{
  "user_id": "user-uuid",
  "username": "new_user",
  "email": "user@example.com",
  "role": "User",
  "status": "Active",
  "access_key": "access-key-here",
  "access_secret": "access-secret-here",
  "created_on": "2024-01-15T10:30:00Z",
  "message": "User registered successfully. Store credentials securely - access_secret won't be shown again"
}
```

## Upload Status Tracking

### Get User Upload Agents
`GET /user/upload-agents`

**Headers**: `Authorization: Bearer <token>`

**Query Parameters**:
- `limit`: Maximum number of agents to return (default: 100)

```json
{
  "data": [
    {
      "agent_name": "my-agent",
      "status": "completed",
      "created_at": "2024-01-15T10:30:00Z",
      "upload_id": "uuid-here"
    }
  ]
}
```

### Update Latest Upload Status
`PUT /upload-status/agent/{agent_name}/latest`

**Description**: Update the latest upload status for an agent (used by orchestrator)

```json
{
  "status": "completed",
  "progress": 100,
  "message": "Deployment successful",
  "details": {}
}
```

## Observability Endpoints

### Get Agent Traces  
`POST /traces`

**Note**: This endpoint is currently handled by the TracesHandler but the route needs to be added to the router.

Request body:
```json
{
  "agent_name": "document-expert",
  "page_size": 10,
  "page": 1
}
```

**Response**:
```json
{
  "data": {
    "traces": [
      {
        "trace_id": "abc123",
        "span_id": "span456",
        "agent_name": "document-expert",
        "operation_name": "process_document",
        "start_time": "2024-01-15T10:30:00Z",
        "end_time": "2024-01-15T10:30:05Z",
        "duration_ms": 5000,
        "status": "success",
        "metadata": {
          "model_used": "gpt-4",
          "tokens_consumed": 1500,
          "cost_usd": 0.03
        }
      }
    ],
    "pagination": {
      "current_page": 1,
      "page_size": 10,
      "total_traces": 150,
      "total_pages": 15,
      "has_next": true,
      "has_previous": false
    }
  }
}
```

## Kong Agent Gateway Integration

The Kong gateway provides agent access via standardized routes:

### Agent Routes
| Agent | Kong Route | Direct URL |
|-------|------------|------------|
| document-expert | `/document-expert/*` | `http://localhost:9100/document-expert/chat` |
| translator | `/translator/*` | `http://localhost:9100/translator/translate` |
| github-agent | `/github-agent/*` | `http://localhost:9100/github-agent/analyze` |
| compliance-checker | `/compliance-checker/*` | `http://localhost:9100/compliance-checker/check` |

### Kong API Endpoints
- **Kong Admin**: `http://localhost:9101/services`
- **Service Registry**: `http://localhost:8080/status`

## Router Service Integration

The Router Service provides intelligent query routing:

### Route Request
`POST http://localhost:8005/route`

```json
{
  "session_id": "session123",
  "query": "Translate this text to Spanish", 
  "has_route": false,
  "route": ""
}
```

### Stream Request
`POST http://localhost:8005/stream`

Multipart form data with streaming response for real-time agent selection.

## Data Models

### Enhanced Registry Model
```python
{
  "id": "string",                     # Database ID
  "agent": {
    "id": "string",                   # Agent unique ID
    "name": "string",                 # Agent name
    "version": "string",              # Version (e.g., "1.0.0")
    "tags": ["string"],               # Categories/tags
    "description": "string"           # Agent description
  },
  "capabilities": [Capability],       # List of capabilities
  "health_check": {                   # Health check config
    "endpoint": "/health",
    "method": "GET", 
    "expected_status": 200
  },
  "agent_url": "string",              # Agent base URL
  "metadata": {                       # Additional metadata
    "created_by": "string",
    "model_used": "string",
    "created_at": "datetime",
    "updated_at": "datetime"
  }
}
```

### Enhanced Capability Model
```python
{
  "id": "string",                     # Capability ID
  "name": "string",                   # Display name
  "description": "string",            # Description
  "endpoint": {
    "path": "string",                 # API path
    "method": "string",               # HTTP method
    "headers": {}                     # Required headers
  },
  "input_schema": {},                 # Input parameters
  "output_schema": {},                # Output format
  "error_handling": {},               # Error responses
  "tags": ["string"]                  # Capability tags
}
```

### Upload Status Model
```python
{
  "upload_id": "string",              # Unique upload ID
  "agent_name": "string",             # Agent name
  "status": "pending|in_progress|completed|failed",
  "progress": "number",               # 0-100
  "message": "string",                # Status message
  "created_at": "datetime",
  "updated_at": "datetime",
  "details": {
    "capabilities_generated": "boolean",
    "orchestration_triggered": "boolean",
    "validation_errors": ["string"]
  }
}
```

### GitHub Repository Model
```python
{
  "name": "string",                   # Repository name
  "full_name": "string",              # Full name (owner/repo)
  "description": "string",            # Repository description
  "clone_url": "string",              # Git clone URL
  "html_url": "string",               # GitHub web URL
  "private": "boolean",               # Is private repository
  "language": "string"                # Primary language
}
```

## Error Codes

- `200` OK - Success
- `201` Created - Resource created
- `307` Temporary Redirect - OAuth redirects
- `400` Bad Request - Invalid input
- `401` Unauthorized - Authentication required
- `404` Not Found - Resource not found
- `422` Validation Error - Data validation failed
- `500` Internal Error - Server error

## Usage Examples

### Python Client
```python
import requests

base_url = "http://localhost:8000/api/v1"

# Upload agent ZIP
with open("agent.zip", "rb") as f:
    files = {"file": f}
    data = {"agent_name": "my-agent"}
    response = requests.post(f"{base_url}/agents/upload", files=files, data=data)
    upload_data = response.json()
    upload_id = upload_data["data"]["upload_id"]

# Track upload status
response = requests.get(f"{base_url}/upload-status/{upload_id}")
status = response.json()

# Create registry
registry_data = {
    "agent": {
        "id": "test-agent-v1",
        "name": "test-agent",
        "version": "1.0.0",
        "tags": ["test"],
        "description": "Test agent"
    },
    "capabilities": [],
    "agent_url": "http://localhost:8001"
}
response = requests.post(f"{base_url}/registries", json=registry_data)

# Access agent via Kong
kong_response = requests.post("http://localhost:9100/test-agent/chat", 
                             json={"message": "Hello"})
```

### cURL Examples
```bash
# Upload agent ZIP
curl -X POST "http://localhost:8000/api/v1/agents/upload" \
  -F "file=@agent.zip" \
  -F "agent_name=my-agent"

# Upload from GitHub
curl -X POST "http://localhost:8000/api/v1/github/clone" \
  -H "Content-Type: application/json" \
  -d '{"repository_url": "https://github.com/user/repo", "agent_name": "github-agent"}'

# Get upload status
curl "http://localhost:8000/api/v1/upload-status/abc-123"

# Get registries
curl "http://localhost:8000/api/v1/registries"

# Get traces
curl -X POST "http://localhost:8000/api/v1/traces" \
  -H "Content-Type: application/json" \
  -d '{"agent_name": "document-expert", "page_size": 5, "page": 1}'

# GitHub auth (redirects to GitHub)
curl "http://localhost:8000/api/v1/auth/github/login?state=unique-state-123"

# Access agent via Kong
curl "http://localhost:9100/document-expert/chat" \
  -H "Content-Type: application/json" \
  -d '{"message": "Analyze this document"}'
```

## Authentication

### GitHub OAuth Flow
1. Call `/auth/github/login?state={unique_state}` (redirects to GitHub)
2. GitHub redirects to `/auth/github/callback` with code
3. Use `/auth/github/token` to get access token
4. Access GitHub APIs with token

### Development vs Production
- **Development**: No authentication required for most endpoints
- **Production**: Implement JWT tokens, API keys, rate limiting

## OpenAPI Specification

- **Interactive Docs**: http://localhost:8000/docs
- **JSON Spec**: http://localhost:8000/openapi.json
- **ReDoc**: http://localhost:8000/redoc

## Client SDK Generation

```bash
# Generate Python client
openapi-generator generate \
  -i http://localhost:8000/openapi.json \
  -g python \
  -o ./nasiko-python-client

# Generate TypeScript client
openapi-generator generate \
  -i http://localhost:8000/openapi.json \
  -g typescript-fetch \
  -o ./nasiko-ts-client
```

---

**Version:** v1.0.0 | **Support:** [GitHub Issues](https://github.com/arithmic/nasiko/issues)