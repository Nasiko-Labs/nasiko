"""
Pydantic request and response schemas for the orchestration API.

The models keep HTTP payload validation separate from orchestration logic.
Additional response schemas can be added later for queued, overflow, or
streaming responses without changing the underlying services.
"""

from pydantic import BaseModel, Field
from typing import Optional


class RequestModel(BaseModel):
    """
    AI processing request payload.
    
    Attributes:
        agent: Name/ID of the target AI agent
        query: User query or task description
    """
    agent: str = Field(..., description="Target AI agent name/ID")
    query: str = Field(..., description="User query or request")

    class Config:
        json_schema_extra = {
            "example": {
                "agent": "coder",
                "query": "Generate a small Python function"
            }
        }


class ResponseModel(BaseModel):
    """
    Synchronous AI processing response.
    
    Attributes:
        agent: Name of the agent that processed the request
        response: Processed response from the AI agent
        cached: Whether the response came from cache
        processing_time: Time taken to process request (seconds)
    """
    agent: str = Field(..., description="AI agent that processed the request")
    response: str = Field(..., description="AI-generated response")
    cached: bool = Field(default=False, description="Whether response was cached")
    processing_time: float = Field(default=0.0, description="Processing time in seconds")

    class Config:
        json_schema_extra = {
            "example": {
                "agent": "translator",
                "response": "Bonjour (FR) / Hola (ES) / Hallo (DE)",
                "cached": False,
                "processing_time": 2.05
            }
        }


class StatusModel(BaseModel):
    """Response model for status endpoints."""
    status: str = Field(..., description="Status message")
    message: Optional[str] = Field(None, description="Additional context")


class HealthModel(BaseModel):
    """Health check response model."""
    status: str = Field(default="healthy", description="Service health status")
    version: str = Field(default="1.0.0", description="API version")
