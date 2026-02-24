# Agent Port Configuration Fix - Implementation Plan

## Problem Statement

Agents are deployed with incorrect port configuration causing 502 Bad Gateway errors:
- Agents run on port **5000** (hardcoded in Dockerfile CMD)
- K8s deployments default to port **8000** (hardcoded in AgentDeployRequest)
- Kong gateway receives 502 errors when trying to route to agents

## Root Cause

The deployment system has no mechanism to extract or use the actual port that agents listen on. Port 8000 is hardcoded in:
- `/app/api/types.py:266` - `AgentDeployRequest` class
- `/app/service/k8s_service.py:125` - `deploy_agent()` method

## Long-Term Solution (Future Implementation)

### Approach: Add port field to AgentCard.json

1. **Add port field to data model** (`/app/entity/entity.py`)
2. **Extract port from docker-compose.yml** during agent upload
3. **Store port in AgentCard.json** as metadata
4. **Read port from AgentCard during K8s deployment**
5. **Maintain backward compatibility** with default port 5000

### Implementation Phases

#### Phase 1: Schema Changes (30 mins)
- Add `port: Optional[int] = 5000` to `RegistryBase` class
- Add `port` to API response types
- Update `AgentDeployRequest` default from 8000 to 5000

#### Phase 2: Port Extraction (1 hour)
- Implement `_extract_port_from_docker_compose()` utility in agent_upload_service.py
- Parse docker-compose.yml `ports:` section
- Handle formats: `"5000"`, `"8080:5000"`, `5000`, `"5000/tcp"`
- Update `_ensure_agentcard_json()` to extract and pass port
- Update `AgentCardService` methods to accept port parameter
- Update `AgentCardGeneratorAgent` to include port in output

#### Phase 3: Deployment Integration (1 hour)
- Update `deploy_agent_container()` to fetch registry and read port
- Implement three-level fallback: request port → registry port → default 5000
- Add detailed logging for port resolution
- Add repository method `get_registry_by_agent_id()` if missing

### Critical Files for Full Implementation

1. `/app/entity/entity.py` - Add port to RegistryBase
2. `/app/service/agent_upload_service.py` - Extract port from docker-compose.yml
3. `/app/service/agentcard_service.py` - Pass port through generation chain
4. `/app/utils/agentcard_generator/tools.py` - Include port in AgentCard.json output
5. `/app/service/agent_operations_service.py` - Read port from registry during deployment

### Error Handling & Fallbacks

**Three-Level Fallback Chain:**
1. Explicit Request: If `deploy_request.port` is set → use it
2. Registry Data: If agent registry has `port` field → use it
3. Default Value: Use 5000 as fallback

**Philosophy:** Never fail deployment due to port configuration issues - always fall back to working default.

### Testing Strategy

- Unit tests for port extraction from docker-compose.yml
- Integration tests for upload → storage → deployment flow
- Manual E2E test: Upload agent → verify AgentCard → deploy → check K8s config
- Verify Kong can reach deployed agent (no 502 errors)

## Quick Fix (Implemented)

**Date:** 2025-12-22

**Change:** Update hardcoded default port from 8000 to 5000 in deployment code

**Files Modified:**
- `/app/api/types.py` - Change `AgentDeployRequest` default port to 5000
- Any other files with hardcoded port 8000

**Impact:**
- New deployments will use port 5000 by default
- Matches actual agent implementations
- Fixes 502 Gateway errors immediately
- Does NOT add port extraction/storage (future work)

**Limitation:** This is a temporary fix. Agents that run on non-standard ports will still need manual configuration.

## Future Enhancements

1. **Port Validation:** Compare Dockerfile port vs docker-compose port, warn on mismatch
2. **Multiple Services:** Handle docker-compose.yml with multiple services
3. **Dynamic Ports:** Parse Dockerfile for environment variable placeholders
4. **AgentCard Regeneration:** Add CLI command to regenerate AgentCards with port field
5. **Port Discovery API:** Add endpoint to query agent port configurations

## References

- A2A Protocol v0.2.9: https://github.com/a2a-ai/a2a-protocol
- AgentCard Specification: Allows custom extension fields
- Kong Service Discovery: Routes by agent ID, not explicit port
