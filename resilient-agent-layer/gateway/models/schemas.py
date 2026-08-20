from pydantic import BaseModel, Field
from typing import Any, Optional


class AgentRequest(BaseModel):
    """Incoming request from the client to a specific agent."""
    agent_id: str = Field(..., description="Target agent identifier")
    payload: Any = Field(..., description="The request body to forward to the agent")
    priority: int = Field(0, ge=0, le=10, description="Request priority (higher = served first)")
    bypass_cache: bool = Field(False, description="Force fresh response, skipping cache")


class AgentResponse(BaseModel):
    """Wrapped response returned to the client."""
    agent_id: str
    source: str  # 'cache' | 'agent' | 'queue'
    latency_ms: float
    data: Any


class RateLimitUpdate(BaseModel):
    rps: Optional[float] = Field(None, gt=0)
    burst: Optional[int] = Field(None, gt=0)
