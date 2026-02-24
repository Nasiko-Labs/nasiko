# Nasiko Router Service

A modular, AI-powered agent routing service that intelligently selects the best agent to handle user queries.

## Architecture Overview

The router service uses a clean, modular architecture with clear separation of concerns:

```
src/
├── config/              # Configuration management
│   ├── settings.py      # Centralized settings with validation
│   └── __init__.py
├── core/                # Core business services
│   ├── agent_registry.py   # Agent registry interactions
│   ├── vector_store.py     # Vector similarity search
│   ├── agent_client.py     # Agent communication
│   └── __init__.py
├── services/            # Orchestration services
│   ├── router_orchestrator.py  # Main workflow coordinator
│   └── __init__.py
├── main.py             # FastAPI application
├── service.py          # Service layer (compatibility)
├── models.py           # Data models
├── utils.py            # Utility functions
└── routing_agent.py    # AI routing logic
```

## Key Features

### 🧠 **Intelligent Agent Selection**
- AI-powered routing using vector similarity search
- Fallback mechanisms for reliability
- Context-aware agent matching

### 🔧 **Modular Architecture**
- **AgentRegistry**: Manages agent discovery and caching
- **VectorStoreService**: Handles similarity search with FAISS
- **AgentClient**: Manages agent communication
- **RouterOrchestrator**: Coordinates the complete workflow

### ⚡ **Performance & Reliability**
- Agent cards caching with configurable TTL
- Vector store caching based on content hash
- Comprehensive error handling and recovery
- Health checks for all components

### 🛠️ **Configuration Management**
- Environment-based configuration with validation
- Type-safe settings using Pydantic
- Sensible defaults for all options

## Configuration

### Environment Variables

The service reads configuration from `.env` files following the same pattern as the main Nasiko application:

```bash
# Required
OPENAI_API_KEY=sk-your-openai-api-key
NASIKO_BACKEND=http://nasiko-backend:8000/api/v1

# Optional (with defaults)
ENV=development
HOST=0.0.0.0
PORT=8000
LOG_LEVEL=INFO
REQUEST_TIMEOUT=60.0
MAX_FILE_SIZE=1073741824
VECTOR_STORE_CACHE_TTL=3600
EMBEDDING_MODEL=text-embedding-ada-002
CORS_ORIGINS=http://localhost:4000,http://127.0.0.1:4000
```

**Configuration Files**: The service looks for `.env` files in multiple locations:
- `.env` (root directory)
- `router/.env` 
- `kong/router/.env`

Copy `.env.example` to `.env` and update with your values.

### Configuration Validation

All settings are validated at startup:
- URLs must start with http/https
- File sizes must be positive
- Log levels must be valid
- Required API keys are checked

## API Endpoints

### POST `/router`
Main routing endpoint that processes user queries.

**Request:**
```bash
curl -X POST http://localhost:8000/router \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -F "session_id=session123" \
  -F "query=Translate this text to Spanish" \
  -F "route=http://agent:8001/chat" \  # Optional direct route
  -F "files=@document.pdf"             # Optional files
```

**Response:**
Streaming JSON responses showing processing steps:
```json
{"message": "Processing user's query...", "is_int_response": true, "agent_id": "", "url": ""}
{"message": "Fetching agent details from registry...", "is_int_response": true, "agent_id": "", "url": ""}
{"message": "Agent selected: translator", "is_int_response": true, "agent_id": "translator", "url": ""}
{"message": "Final response from agent", "is_int_response": false, "agent_id": "", "url": "http://agent:8001"}
```

### GET `/health`
Health check endpoint returning component status.

**Response:**
```json
{
  "router": "healthy",
  "timestamp": 1701234567.89,
  "components": {
    "vector_store": "healthy",
    "agent_registry": "healthy", 
    "agent_client": "healthy"
  }
}
```

### GET `/metrics`
Service metrics endpoint (TODO: implementation pending).

## Usage Examples

### Basic Query Routing
```python
import httpx

async def route_query():
    async with httpx.AsyncClient() as client:
        response = await client.post(
            "http://localhost:8000/router",
            headers={"Authorization": "Bearer YOUR_TOKEN"},
            data={
                "session_id": "user123",
                "query": "Analyze this code repository"
            }
        )
        
        async for line in response.aiter_lines():
            print(line)
```

### Direct Agent Routing
```python
# Skip agent selection, route directly
response = await client.post(
    "http://localhost:8000/router",
    headers={"Authorization": "Bearer YOUR_TOKEN"},
    data={
        "session_id": "user123", 
        "query": "Hello",
        "route": "http://github-agent:8002/chat"
    }
)
```

### File Upload with Routing
```python
files = [("files", ("document.pdf", pdf_content, "application/pdf"))]

response = await client.post(
    "http://localhost:8000/router",
    headers={"Authorization": "Bearer YOUR_TOKEN"},
    data={
        "session_id": "user123",
        "query": "Summarize this document"
    },
    files=files
)
```

## Development

### Setup
```bash
# Install dependencies
poetry install

# Run in development mode
uvicorn main:app --reload --host 0.0.0.0 --port 8000

# Run tests
pytest tests/
```

### Adding New Services

1. **Create service in `core/`**:
```python
# core/new_service.py
class NewService:
    def __init__(self):
        pass
    
    async def process(self, data):
        # Implementation
        pass
```

2. **Add to orchestrator**:
```python
# services/router_orchestrator.py
from router.src.core import NewService

class RouterOrchestrator:
    def __init__(self):
        # ...
        self.new_service = NewService()
```

3. **Update configuration if needed**:
```python
# config/settings.py
class RouterConfig(BaseSettings):
    # Add new settings
    new_service_enabled: bool = True
```

## Error Handling

The service provides comprehensive error handling:

### Service-Specific Exceptions
- `AgentRegistryError`: Registry communication failures
- `VectorStoreError`: Vector store operation failures  
- `AgentClientError`: Agent communication failures

### Error Response Format
```json
{
  "message": "Detailed error description",
  "is_int_response": false,
  "agent_id": "",
  "url": ""
}
```

### Retry and Fallback Strategies
- Automatic fallback to alternative agents
- Registry response caching for resilience
- Graceful degradation when services are unavailable

## Monitoring

### Health Checks
- Component-level health status
- Dependency health validation
- Startup configuration validation

### Logging
- Structured JSON logging
- Configurable log levels
- Request tracing and correlation

### Metrics (Planned)
- Request rate and latency
- Agent selection success rate
- Cache hit ratios
- Error rates by component

## Docker Usage

### Build
```bash
docker build -t nasiko-router .
```

### Run
```bash
docker run -p 8000:8000 \
  -e OPENAI_API_KEY=your-key \
  -e NASIKO_BACKEND=http://backend:8000/api/v1 \
  nasiko-router
```

### Environment Integration
The service integrates seamlessly with the Nasiko platform:
- Connects to Kong gateway network
- Communicates with agent registry
- Forwards requests to deployed agents

## Performance

### Caching Strategy
- **Agent Cards**: Cached with configurable TTL (default 1 hour)
- **Vector Stores**: Cached based on agent cards content hash
- **Embeddings**: Reused across requests when agent set unchanged

### Optimization Tips
- Set `VECTOR_STORE_CACHE_TTL` based on agent update frequency
- Use appropriate `REQUEST_TIMEOUT` for agent response times
- Monitor cache hit rates in production

## Troubleshooting

### Common Issues

**"No agents available in registry"**
- Check NASIKO_BACKEND URL configuration
- Verify authentication token is valid
- Ensure agent registry is populated

**"Failed to create vector store"**
- Verify OPENAI_API_KEY is set correctly
- Check OpenAI API quota and rate limits
- Ensure agent descriptions are not empty

**"Agent communication timeout"**
- Increase REQUEST_TIMEOUT setting
- Check agent container health
- Verify network connectivity to agents

### Debug Mode
```bash
# Enable debug logging
LOG_LEVEL=DEBUG uvicorn main:app --reload
```

### Health Check
```bash
# Quick health check
curl http://localhost:8000/health | jq
```

## Contributing

1. Follow the modular architecture patterns
2. Add comprehensive error handling
3. Include unit tests for new services
4. Update configuration documentation
5. Maintain backward compatibility

---

**Version**: 2.0.0  
**License**: MIT  
**Maintainer**: Nasiko Team