"""
N8N Entity - Unified models for N8N integration and user credentials
"""

from pydantic import BaseModel, Field
from typing import Optional, Dict, Any, List
from datetime import datetime, timezone

class UserN8NCredentialCreateRequest(BaseModel):
    """Request to create user N8N credential"""
    connection_name: str = Field(..., description="User-defined connection name")
    n8n_url: str = Field(..., description="N8N instance URL")
    api_key: str = Field(..., description="N8N API key")
    
class UserN8NCredentialUpdateRequest(BaseModel):
    """Request to update user N8N credential"""
    connection_name: Optional[str] = Field(None, description="User-defined connection name")
    n8n_url: Optional[str] = Field(None, description="N8N instance URL")
    api_key: Optional[str] = Field(None, description="N8N API key")
    is_active: Optional[bool] = Field(None, description="Whether credential is active")

class UserN8NCredentialTestRequest(BaseModel):
    """Request to test user N8N credential connection"""

class UserN8NCredentialResponse(BaseModel):
    """Response for user N8N credential operations"""
    success: bool
    message: str
    user_id: Optional[str] = None
    connection_name: Optional[str] = None
    n8n_url: Optional[str] = None
    is_active: Optional[bool] = None
    last_tested: Optional[datetime] = None
    created_at: Optional[datetime] = None
    updated_at: Optional[datetime] = None

class UserN8NCredentialSingleResponse(BaseModel):
    """Single N8N credential response wrapper"""
    success: bool
    message: str
    data: Optional[UserN8NCredentialResponse] = None

class UserN8NConnectResponse(BaseModel):
    """Simplified response for N8N connect endpoint"""
    data: dict
    status_code: int
    message: str

# Workflow Models
class WorkflowSummary(BaseModel):
    """Simplified workflow information for UI listing"""
    id: str
    name: str
    active: bool
    is_chat_workflow: bool = False
    nodes_count: int = 0
    last_updated: Optional[str] = None
    tags: List[str] = []

class WorkflowListResponse(BaseModel):
    """Response containing list of workflows"""
    workflows: List[WorkflowSummary]
    total_count: int
    connection_name: Optional[str] = None
    message: str

# N8N Registration Models
class N8nRegisterRequest(BaseModel):
    workflow_id: str = Field(..., description="N8n workflow ID to register")
    agent_name: Optional[str] = Field(None, description="Custom agent name (auto-generated if not provided)")
    agent_description: Optional[str] = Field(None, description="Custom agent description")

class N8nRegisterResponse(BaseModel):
    success: bool
    message: str
    agent_name: Optional[str] = None
    agent_id: Optional[str] = None
    webhook_url: Optional[str] = None
    container_name: Optional[str] = None
    upload_id: Optional[str] = None