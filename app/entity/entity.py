from datetime import datetime, timezone
from enum import Enum
from typing import List, Optional, Any, Dict
from uuid import UUID, uuid4

from bson import ObjectId
from pydantic import BaseModel, Field, GetCoreSchemaHandler
from pydantic_core import core_schema


# Any entity models here ..
# Custom ObjectId type for Pydantic v2
class PyObjectId(ObjectId):
    @classmethod
    def __get_pydantic_core_schema__(
        cls, source_type: Any, handler: GetCoreSchemaHandler
    ) -> core_schema.CoreSchema:
        return core_schema.no_info_after_validator_function(
            cls.validate,
            core_schema.any_schema(),
            serialization=core_schema.to_string_ser_schema(),
        )

    @classmethod
    def validate(cls, v):
        if isinstance(v, ObjectId):
            return str(v)  # Convert to string immediately
        if isinstance(v, str) and ObjectId.is_valid(v):
            return v  # Keep as string
        raise ValueError("Invalid ObjectId")

    @classmethod
    def __get_pydantic_json_schema__(cls, core_schema, handler):
        return handler(core_schema)

class Skill(BaseModel):
    id: str
    name: str
    description: str
    tags: List[str] = []
    examples: List[Any] = []  # Accept any format (strings or objects)


class Provider(BaseModel):
    organization: str
    url: Optional[str] = None


class Capabilities(BaseModel):
    streaming: bool = False
    pushNotifications: bool = False
    stateTransitionHistory: bool = False
    chat_agent: bool = False


class RegistryBase(BaseModel):
    # AgentCard format fields
    protocolVersion: str = "0.2.9"
    id: str  # Agent identifier
    name: str
    description: str
    url: str  # Deployment URL where agent is accessible
    preferredTransport: str = "JSONRPC"
    provider: Optional[Provider] = None
    iconUrl: Optional[str] = None
    version: str = "1.0.0"
    documentationUrl: Optional[str] = None
    capabilities: Capabilities = Field(default_factory=Capabilities)
    securitySchemes: Dict[str, Any] = {}
    security: List[Any] = []
    defaultInputModes: List[str] = ["application/json", "text/plain"]
    defaultOutputModes: List[str] = ["application/json"]
    skills: List[Skill] = []
    supportsAuthenticatedExtendedCard: bool = False
    signatures: List[Any] = []
    additionalInterfaces: Optional[List[Dict[str, str]]] = None
    
    # Combined tags from all skills (deduplicated)
    tags: List[str] = []

    # Owner information
    owner_id: str

    # Timestamps
    created_at: datetime = Field(default_factory=lambda: datetime.now(timezone.utc))
    updated_at: datetime = Field(default_factory=lambda: datetime.now(timezone.utc))


class RegistryInDB(RegistryBase):
    # MongoDB document ID (separate from agent ID) 
    _id: Optional[str] = None

    model_config = {
        "json_encoders": {ObjectId: str, datetime: str, PyObjectId: str},
        "populate_by_name": True,
    }


# Upload Status Tracking Models
class UploadStatus(str, Enum):
    INITIATED = "initiated"
    PROCESSING = "processing"
    CAPABILITIES_GENERATED = "capabilities_generated"
    ORCHESTRATION_TRIGGERED = "orchestration_triggered"
    ORCHESTRATION_PROCESSING = "orchestration_processing"
    COMPLETED = "completed"
    FAILED = "failed"

class BuildStatus(str, Enum):
    QUEUED = "queued"
    BUILDING = "building"
    SUCCESS = "success"
    FAILED = "failed"

class AgentBuildBase(BaseModel):
    agent_id: str  # Reference to the Registry ID
    github_url: Optional[str] = None
    commit_hash: Optional[str] = None
    version_tag: str # e.g., v1.0.0
    image_reference: str # e.g., harbor.nasiko.io/agents/my-agent:v1.0.0

    status: BuildStatus = BuildStatus.QUEUED
    k8s_job_name: Optional[str] = None
    logs: Optional[str] = None

    created_at: datetime = Field(default_factory=lambda: datetime.now(timezone.utc))
    updated_at: datetime = Field(default_factory=lambda: datetime.now(timezone.utc))

class AgentBuildInDB(AgentBuildBase):
    id: str = Field(default_factory=lambda: str(ObjectId()), alias="_id")

class DeploymentStatus(str, Enum):
    STARTING = "starting"
    RUNNING = "running"
    STOPPED = "stopped"
    FAILED = "failed"

class AgentDeploymentBase(BaseModel):
    id: str = Field(default_factory=lambda: str(ObjectId()), alias="_id")
    agent_id: str
    build_id: str # Link to the specific build used
    namespace: str = "nasiko-agents"
    replicas: int = 1
    status: DeploymentStatus = DeploymentStatus.STARTING
    service_url: Optional[str] = None # The internal K8s DNS

    created_at: datetime = Field(default_factory=lambda: datetime.now(timezone.utc))

class CreateSessionRequest(BaseModel):
    agent_id: Optional[str] = Field(None, description="Optional agent ID to associate with session")
    agent_url: Optional[str] = Field(None, description="Optional agent URL for direct communication")


class SessionData(BaseModel):
    session_id: str = Field(..., description="Unique session identifier")
    created_at: str = Field(..., description="Session creation timestamp")
    title: str = Field(..., description="Random session title")
    agent_id: Optional[str] = Field(None, description="Associated agent ID")
    agent_url: Optional[str] = Field(None, description="Associated agent URL")


class SessionResponse(BaseModel):
    data: SessionData
    status_code: int = 201
    message: str = "Session created successfully"


class MessageResponse(BaseModel):
    status_code: int = Field(default=200, description="HTTP status code")
    message: str = Field(..., description="Response message")


class PaginationMetaData(BaseModel):
    total_count: int = Field(..., description="Total number of records")
    returned_count: int = Field(..., description="Number of messages in this response")
    has_more: bool = Field(..., description="Whether more messages exist")
    next_cursor: Optional[str] = Field(None, description="Cursor for next page")
    prev_cursor: Optional[str] = Field(None, description="Cursor for previous page")


class SessionHistory(BaseModel):
    session_id: str = Field(..., description="Unique session identifier")
    title: str = Field(..., description="Session title")
    agent_id: Optional[str] = Field(None, description="Associated agent ID")
    agent_url: Optional[str] = Field(None, description="Associated agent URL")


class SessionHistoryResponse(BaseModel):
    data: List[SessionHistory]
    pagination: Optional[PaginationMetaData] = None
    status_code: int = 200
    message: str = "Session history retrieved successfully"


class ChatHistory(BaseModel):
    role: str = Field(..., description="User or Assistant")
    content: str = Field(..., description="Chat's content")
    timestamp: datetime = Field(..., description="Chat's timestamp")


class ChatHistoryResponse(BaseModel):
    data: List[ChatHistory]
    pagination: Optional[PaginationMetaData] = None
    status_code: int = 200
    message: str = "Chat history retrieved successfully"
