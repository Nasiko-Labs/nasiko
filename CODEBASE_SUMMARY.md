# Nasiko Platform - Comprehensive Codebase Summary

## Executive Summary

**Nasiko** is an enterprise-grade multi-agent platform that intelligently routes user requests to specialized AI agents. The system combines:
- **Smart routing** using LLM + semantic similarity
- **Kubernetes-based deployment** for scalability
- **Event-driven orchestration** via Redis streams
- **Observable agents** with OpenTelemetry instrumentation
- **Multi-LLM support** (OpenAI, OpenRouter, MiniMax, local Ollama)

---

## Architecture Overview

### Three Core Services

#### 1. **Agent Gateway Router** (`agent-gateway/router/`)
- **Purpose**: Intelligent request routing to agents
- **Framework**: FastAPI
- **Key Logic**:
  1. Fetch agent cards from registry (cached 1 hour)
  2. Semantic search with FAISS vectors
  3. LLM-based agent selection (GPT-4o-mini)
  4. Route request to selected agent
  5. Stream response back

**Key URL Translation**: Routes requests to internal Docker network via Kong gateway

#### 2. **Backend Application** (`app/`)
- **Purpose**: Agent management, registry, orchestration
- **Pattern**: Repository → Service → Handler → Routes
- **Database**: MongoDB (async Motor driver)
- **Cache Layer**: Redis for tokens, search indexes, status
- **Key Responsibilities**:
  - Agent registry management
  - Build/deployment tracking
  - Chat history storage
  - File upload processing
  - Kubernetes job creation

#### 3. **Orchestrator** (`orchestrator/`)
- **Purpose**: Build and deploy agents
- **Architecture**: Async event listener for Redis streams
- **Flow**:
  1. Listen on Redis stream `orchestration:commands`
  2. Validate agent structure
  3. Inject observability instrumentation
  4. Build Docker image with BuildKit
  5. Deploy to Kubernetes
  6. Update registry

---

## Request Flow Analysis

### Chat/Query Request Journey

```
User Request
  ↓
Kong Gateway (API proxy, URL routing)
  ↓
Router Service (/router endpoint)
  ├─ Input validation
  ├─ File processing
  ├─ Fetch agent cards (cached)
  ├─ FAISS semantic search
  ├─ LLM routing decision
  ├─ URL translation
  └─ HTTP request to agent
  ↓
Selected Agent (Docker container in K8s)
  ↓
Response streaming back to user
```

**Cache Optimization**: Agent registry cached for 3600s (configurable via `VECTOR_STORE_CACHE_TTL`)

### Agent Upload & Deployment Journey

```
Admin uploads agent ZIP
  ↓
Backend: AgentUploadService
  ├─ Extract & validate
  ├─ Generate AgentCard.json
  ├─ Create registry entry
  └─ Send command to Redis stream
  ↓
Redis Stream: orchestration:commands
  ↓
Orchestrator Service (separate process)
  ├─ Read stream message
  ├─ Validate agent structure
  ├─ Inject instrumentation
  ├─ Trigger BuildKit job in K8s
  ├─ Build Docker image
  ├─ Deploy to K8s
  └─ Update registry & status
  ↓
Agent running in K8s, visible in registry
```

**Async Benefit**: Upload returns immediately while orchestration happens independently

---

## Technology Stack Analysis

### Core Framework
- **FastAPI** (>=0.116.1) - Web framework with built-in async
- **uvicorn** - ASGI server
- **Pydantic** (>=2.11.7) - Data validation with type hints

### Data Persistence
- **MongoDB** (4.8.0) - Document store via Motor (async driver)
- **Redis** (>=6.4.0) - Cache, sessions, streams
- Built-in Mongoose-like schema with TTL indexes

### AI/ML Stack
- **LangChain** (>=0.3.27) - LLM orchestration framework
- **FAISS** (^1.8.0) - Vector similarity search (1M+ agents possible)
- **OpenAI Embeddings** - Default embedding provider
- **Jina Embeddings** - Alternative for different use cases
- **LLM Options**: OpenAI, OpenRouter, MiniMax, Ollama

### Observability
- **OpenTelemetry** - Distributed tracing standard
- **Arize Phoenix** (>=12.0.0) - Trace visualization UI
- **OpenInference** - LLM tracing standards

### Infrastructure
- **Kubernetes** (>=33.1.0) - Pod orchestration
- **BuildKit** - Docker image building in K8s
- **HTTPx** / **aiohttp** - Async HTTP clients

---

## Caching Mechanisms

### 1. **Agent Registry Cache** (Router)
- **Type**: In-memory Python dict with timestamp
- **TTL**: Configurable via `VECTOR_STORE_CACHE_TTL` (default: 3600 seconds)
- **Hit Path**: Skip HTTP round-trip to backend
- **Invalidation**: Time-based (automatic refresh on TTL)

```python
class AgentRegistry:
    def _is_cache_valid(self) -> bool:
        cache_age = time.time() - self._cache_timestamp
        return cache_age < settings.VECTOR_STORE_CACHE_TTL
```

### 2. **Vector Store Cache** (Router)
- **Type**: FAISS vector store (in-memory)
- **Usage**: Semantic similarity for agent selection
- **TTL**: Same as registry cache

### 3. **Redis Data Structures** (Backend)
- **GitHub tokens**: `github_access_token` (simple string)
- **Search indexes**: Sorted sets for user/agent discovery
- **Agent status**: Hash map `agent:status:{name}`
- **Stream persistence**: `orchestration:commands` (auto-trimmed)

### 4. **Database Indexes** (MongoDB)
- Automatic index creation on:
  - `user_id` (sessions, uploads)
  - `agent_name` (registry)
  - `created_at`, `updated_at` (timestamps)
  - Status fields for querying

### Rate Limiting
**No explicit rate limiting** at application level. Likely handled at:
- Kong gateway (proxy-level rate limiting)
- Auth service (JWT validation throttling)
- Cloud infrastructure (auto-scaling)

---

## Communication Patterns

### Synchronous (HTTP)
- **Client ↔ Kong**: REST API calls
- **Router ↔ Backend**: Fetch agent cards, registry updates
- **Router ↔ Agents**: Route incoming requests
- **Backend ↔ Auth**: Token validation

### Asynchronous (Redis Streams)
- **Backend → Orchestrator**: Command stream `orchestration:commands`
- **Status Updates**: Redis hashes for agent status
- **Typical Latency**: Sub-second for stream delivery

### WebSocket / Streaming
- **Router Response**: AsyncGenerator for streaming updates
- **File Uploads**: Multipart form-data with file chunks

---

## Dependency Injection Pattern

```python
# In FastAPI routes
@router.get("/agents")
async def get_agents(
    handlers: HandlerFactory = Depends(providers.get_handlers),
    user_id: str = Depends(get_user_id_from_token)
):
    return await handlers.registry.get_agents(user_id)
```

**Inversion of Control**: Dependencies injected through FastAPI's `Depends()`

---

## Error Handling Strategy

### HTTP Status Codes
- **400** - Validation error (invalid input)
- **401** - Auth error (invalid/expired token)
- **403** - Permission error (not authorized)
- **404** - Not found
- **500** - Server error
- **503** - Service unavailable (auth service down)

### Custom Exceptions
```python
class AgentRegistryError(Exception): pass
class AgentClientError(Exception): pass
class RoutingEngineError(Exception): pass
```

### Error Propagation
- Handler catches and logs
- Returns HTTPException with status code
- FastAPI handles response formatting

---

## Configuration Management

### Environment-Based
```python
# app/pkg/config/config.py - Pydantic settings
class Config(BaseSettings):
    MONGO_URI: str       # Connection string
    REDIS_HOST: str      # Cache/stream location
    K8S_ENABLED: bool    # Toggle Kubernetes
    REGISTRY_URL: str    # Docker image registry
    NASIKO_API_URL: str  # For orchestrator callbacks
    
    model_config = {
        "env_file": [".env", "app/.env"],
        "env_file_encoding": "utf-8",
        "case_sensitive": False,
    }
```

### Router Configuration
```python
# agent-gateway/router/src/config/settings.py
class RouterConfig(BaseSettings):
    NASIKO_BACKEND: str              # Backend API
    ROUTER_LLM_PROVIDER: str         # "openai" (default)
    ROUTER_LLM_MODEL: str            # "gpt-4o-mini"
    EMBEDDING_MODEL: str             # "text-embedding-3-small"
    VECTOR_STORE_CACHE_TTL: int      # 3600 seconds
    MAX_CONCURRENT_REQUESTS: int     # 10
    REQUEST_TIMEOUT: float           # 60.0 seconds
```

---

## Security Architecture

### Authentication
- **JWT Tokens**: Bearer tokens in Authorization header
- **Validation**: External auth service endpoint
- **Subject Types**: "user" or other (extensible)

### Authorization
- **User Isolation**: All queries scoped to authenticated user
- **Session Ownership**: Sessions tied to user_id
- **Token Forwarding**: Router forwards auth to agents

### Secure Defaults
- Session middleware with `httponly` flags
- HTTPS recommended in production (`https_only` setting)
- Encryption for sensitive data (N8N API keys)

---

## Scaling Considerations

### Horizontal Scaling (Load Balancing)
- **Router Service**: Stateless (cache shared via Redis)
- **Backend**: Stateless (MongoDB connection pooling)
- **Orchestrator**: Single instance (consumer group ensures one processor)

### Vertical Scaling (Resource Limits)
- **Max concurrent requests**: 10 (configurable)
- **Request timeout**: 60 seconds (configurable)
- **File size limit**: 1 GB (configurable)

### Vector Store Scaling
- **FAISS**: In-memory (scales to ~100k agents on 16GB RAM)
- **Cache TTL**: 1 hour (adjust for agent count)
- **Semantic search**: Sub-millisecond for typical agent counts

### Database Scaling
- **MongoDB**: Supports sharding for large collections
- **Indexes**: Auto-created on essential fields
- **TTL Indexes**: Auto-cleanup for time-sensitive data

---

## Observability Implementation

### Distributed Tracing
```python
# All services inject OpenTelemetry spans
from opentelemetry import trace

tracer = trace.get_tracer(__name__)
with tracer.start_as_current_span("operation_name"):
    # Business logic here
    pass
```

### Trace Collection
- **Exporter**: OTLP (OpenTelemetry Protocol)
- **Receiver**: Phoenix service (UI at `/traces`)
- **Instrumentation**: 
  - FastAPI requests/responses
  - LangChain operations
  - OpenAI API calls
  - Database operations

### Logging
```python
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
)
```

---

## File Organization

```
nasiko/
├── agent-gateway/router/          # Intelligent router service
│   └── src/
│       ├── main.py               # FastAPI app
│       ├── config/settings.py    # Configuration
│       ├── services/
│       │   └── router_orchestrator.py
│       └── core/                 # Routing logic
│           ├── agent_registry.py   (fetch, cache)
│           ├── routing_engine.py   (semantic + LLM)
│           ├── agent_client.py     (send requests)
│           └── vector_store.py     (FAISS)
│
├── app/                           # Main backend
│   ├── main.py                   # FastAPI app + lifespan
│   ├── api/
│   │   ├── routes/               # Modular endpoints
│   │   ├── handlers/             # Request handlers
│   │   ├── auth.py              # JWT validation
│   │   └── types.py             # Pydantic models
│   ├── service/                  # Business logic
│   │   ├── agent_operations_service.py
│   │   ├── agent_upload_service.py
│   │   ├── orchestration_service.py
│   │   └── ...
│   ├── repository/               # Data access
│   │   ├── repository.py          (facade)
│   │   ├── registry_repository.py
│   │   └── ...
│   ├── entity/                   # Pydantic models
│   └── pkg/
│       ├── config/config.py
│       ├── redisclient/
│       └── auth/
│
├── orchestrator/                 # Build/deploy service
│   ├── redis_stream_listener.py  (event listener)
│   ├── agent_builder.py          (build logic)
│   ├── instrumentation_injector.py
│   └── registry_manager.py
│
└── tests/                        # Test suites
```

---

## Key Architectural Decisions

### 1. **Multi-Service Architecture**
**Decision**: Separate router from backend
**Rationale**: 
- Router is stateless (scales horizontally)
- Backend manages state (builds, deployments)
- Clear separation of concerns

### 2. **Event-Driven Orchestration**
**Decision**: Use Redis streams for deployment commands
**Rationale**:
- Backend doesn't block on slow builds
- Orchestrator can run independently
- Easy to replay/retry commands

### 3. **Caching Over Consensus**
**Decision**: Simple TTL caching vs. distributed consensus
**Rationale**:
- 1 hour cache works for agent discovery (agents deployed hourly)
- No complex cache invalidation needed
- Simple to implement and debug

### 4. **LLM + Semantic Hybrid Routing**
**Decision**: Combine vector similarity + LLM reasoning
**Rationale**:
- Vectors fast for initial shortlist (< 15 agents)
- LLM understands nuanced user intent
- Semantic search as pre-filter for efficiency

### 5. **Repository Pattern**
**Decision**: Abstract data access layer
**Rationale**:
- Easy to mock for testing
- Can swap MongoDB for other databases
- Testable business logic

---

## Known Limitations & Gaps

1. **No Explicit Rate Limiting**
   - Handled at proxy layer (Kong)
   - Consider adding token bucket algorithm if needed

2. **Single Orchestrator Instance**
   - Redis consumer group ensures one processor
   - Redeploy for upgrades (brief downtime)

3. **No Distributed Tracing Context Propagation**
   - Traces collected but not fully linked across services
   - Consider OpenTelemetry context propagation headers

4. **Vector Store In-Memory**
   - Scales to ~100k agents
   - Consider Pinecone/Weaviate for millions of agents

5. **No Request Deduplication**
   - Same query can be routed to same agent multiple times
   - Add request ID tracking for idempotency if needed

---

## Testing Strategy

### Unit Tests
- Handler logic with mocked services
- Service business logic with mocked repos
- Pydantic model validation

### Integration Tests
- Full request flow (router → agent)
- Database operations with test MongoDB
- Redis stream ordering

### E2E Tests
- Upload agent → Deploy → Route request
- Full lifecycle testing with real containers

---

## Performance Profiling Opportunities

### Quick Wins
1. **Agent Card Parsing**: Cache serialized cards (not just list)
2. **Vector Embeddings**: Pre-compute on upload (not per-request)
3. **LLM Request Batching**: Group similar queries
4. **Database Query Optimization**: Add compound indexes

### Medium-term
1. **Redis Persistence**: AOF for critical state
2. **Connection Pooling**: Explicit pool for MongoDB/Redis
3. **Async Context**: Fully async stack (no sync I/O)

---

## Deployment Architecture

### Prerequisites
- **Kubernetes 1.24+** - For pod orchestration
- **MongoDB 4.4+** - Document database
- **Redis 6.2+** - Caching and streams
- **Kong API Gateway** - Request routing (optional but recommended)

### Recommended Stack
```yaml
Services:
  - agent-gateway/router       (port 8000)
  - app (backend)              (port 8000)
  - orchestrator               (no port, event listener)
  - redis-stream-listener      (no port, event listener)
  
External:
  - Kong Gateway               (port 8000, proxies to router/backend)
  - MongoDB                    (port 27017)
  - Redis                      (port 6379)
  - Phoenix Observability      (port 6006)
  - Kubernetes Cluster         (k8s API)
```

---

## Conclusion

Nasiko is a sophisticated platform demonstrating:
- ✅ Clean architecture (Repository → Service → Handler → Routes)
- ✅ Event-driven async processing (Redis streams)
- ✅ Intelligent AI routing (LLM + semantics)
- ✅ Infrastructure as Code (Kubernetes + Docker)
- ✅ Observable systems (OpenTelemetry)
- ✅ Scalable design (stateless services)

The codebase prioritizes maintainability, testability, and extensibility while handling the complexity of multi-agent orchestration at scale.

---

**Last Updated**: May 9, 2026  
**Documentation Version**: 1.0
