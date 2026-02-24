from typing import List, Optional, Any, Dict, Union
from pydantic import BaseModel
from app.entity.entity import RegistryBase

# Registry API Types
class RegistryCreateRequest(RegistryBase):
    pass

class RegistryUpsertRequest(RegistryBase):
    pass

class RegistryItemResponse(BaseModel):
    id: str  # Agent ID from AgentCard
    db_id: Optional[str] = None  # Database ID (_id)
    name: str
    version: str
    description: str
    url: str
    preferredTransport: str = "JSONRPC"
    capabilities: Dict[str, Any] = {}
    skills: List[Dict[str, Any]] = []
    defaultInputModes: List[str] = []
    defaultOutputModes: List[str] = []


class RegistryResponse(BaseModel):
    data: List[RegistryItemResponse]
    status_code: int
    message: str


class RegistryItemDetailResponse(BaseModel):
    id: str  # Agent ID from AgentCard
    name: str
    version: str
    description: str
    url: str
    preferredTransport: str = "JSONRPC"
    protocolVersion: str = "0.2.9"
    provider: Optional[Dict[str, Any]] = None
    iconUrl: Optional[str] = None
    documentationUrl: Optional[str] = None
    capabilities: Dict[str, Any] = {}
    securitySchemes: Dict[str, Any] = {}
    security: List[Any] = []
    defaultInputModes: List[str] = []
    defaultOutputModes: List[str] = []
    skills: List[Dict[str, Any]] = []
    tags: List[str] = []  # Combined tags from all skills (deduplicated)
    supportsAuthenticatedExtendedCard: bool = False
    signatures: List[Any] = []
    additionalInterfaces: Optional[List[Dict[str, str]]] = None
    created_at: Optional[str] = None
    updated_at: Optional[str] = None


class RegistrySingleResponse(BaseModel):
    data: RegistryItemDetailResponse
    status_code: int
    message: str


# Generic API Response Types
class SuccessResponse(BaseModel):
    message: str
    success: bool = True

class TraceData(BaseModel):
    name: str
    trace_id: str
    span_id: str
    trace_state: str
    kind: str
    parent_id: str
    start_time: str
    end_time: str
    attributes: str  # JSON string
    status_code: str
    events: str
    links: str
    duration: str


class TokenUsage(BaseModel):
    input_tokens: int
    output_tokens: int
    cached_input_tokens: Optional[int] = None
    total_tokens: int


class TraceNode(BaseModel):
    trace: TraceData
    children: List["TraceNode"] = []
    cost: Optional[float] = None
    tokens: Optional[TokenUsage] = None
    input_cost: Optional[float] = None
    output_cost: Optional[float] = None
    cached_input_cost: Optional[float] = None
    total_cost: Optional[float] = None


class TracesMetadata(BaseModel):
    page: int
    page_size: int
    total_pages: int


class GetTracesResponse(BaseModel):
    traces: List[TraceNode]
    metadata: TracesMetadata


class GetTracesRequest(BaseModel):
    agent_name: str
    page_size: Optional[int] = 10
    page: Optional[int] = 1


class AgentUploadItemResponse(BaseModel):
    success: bool
    agent_name: str
    status: str
    capabilities_generated: bool
    orchestration_triggered: bool
    validation_errors: Optional[List[str]] = None
    version: Optional[str] = None


class AgentUploadResponse(BaseModel):
    data: AgentUploadItemResponse
    status_code: int
    message: str


class AgentDirectoryUploadRequest(BaseModel):
    directory_path: str
    agent_name: Optional[str] = None


# User Management Types
class UserRegistrationRequest(BaseModel):
    username: str
    email: str
    is_super_user: Optional[bool] = False


class UserRegistrationResponse(BaseModel):
    user_id: str
    username: str
    email: str
    role: str  # "User" or "Super User"
    status: str  # "Active" or "Inactive"
    access_key: str
    access_secret: str
    created_on: str
    message: str


# Github API Types
class GithubUser(BaseModel):
    login: str
    id: int
    avatar_url: str
    name: Optional[str] = None
    email: Optional[str] = None


class Token(BaseModel):
    access_token: str
    token_type: str



class GithubLoginResponse(BaseModel):
    user: GithubUser
    token: Token


# GitHub Repository Types
class GithubRepository(BaseModel):
    id: int
    name: str
    full_name: str
    description: Optional[str] = None
    private: bool
    clone_url: str
    ssh_url: str
    html_url: str
    default_branch: str
    updated_at: str


class GithubRepositoryListResponse(BaseModel):
    repositories: List[GithubRepository]
    total: int


class GithubCloneRequest(BaseModel):
    repository_full_name: str
    branch: Optional[str] = None
    agent_name: Optional[str] = None


# Upload Status API Types
class UploadStatusItemResponse(BaseModel):
    upload_id: str
    agent_name: str
    status: str
    progress_percentage: int
    owner_id: Optional[str] = None
    source_info: Optional[Dict[str, Any]] = {}
    file_size: Optional[int] = None
    capabilities_generated: bool = False
    orchestration_triggered: bool = False
    registry_updated: bool = False
    url: Optional[str] = None
    registry_id: Optional[str] = None
    status_message: Optional[str] = None
    error_details: Optional[List[str]] = []
    validation_errors: Optional[List[str]] = []
    created_at: str
    updated_at: str
    completed_at: Optional[str] = None
    processing_duration: Optional[float] = None
    orchestration_duration: Optional[float] = None


class UploadStatusResponse(BaseModel):
    data: List[UploadStatusItemResponse]
    status_code: int
    message: str


class UploadStatusSingleResponse(BaseModel):
    data: UploadStatusItemResponse
    status_code: int
    message: str


class UploadStatusListResponse(BaseModel):
    statuses: List[UploadStatusItemResponse]
    total: int


class UploadStatusUpdateRequest(BaseModel):
    status: Optional[str] = None
    progress_percentage: Optional[int] = None
    status_message: Optional[str] = None
    url: Optional[str] = None
    registry_id: Optional[str] = None
    capabilities_generated: Optional[bool] = None
    orchestration_triggered: Optional[bool] = None
    registry_updated: Optional[bool] = None
    error_details: Optional[List[str]] = None
    validation_errors: Optional[List[str]] = None
    processing_duration: Optional[float] = None
    orchestration_duration: Optional[float] = None

class AgentBuildRequest(BaseModel):
    agent_id: str  # The ID of the agent in the registry
    github_url: str
    version_tag: str # e.g. "v1.0.0"

class AgentDeployRequest(BaseModel):
    agent_id: str
    build_id: str # The specific build ID to deploy
    port: int = 5000 # Port the agent listens on (changed from 8000 to match actual agent implementations)
    env_vars: Optional[Dict[str, str]] = None # Optional environment variables for the agent

class AgentBuildStatusUpdateRequest(BaseModel):
    agent_id: str
    github_url: Optional[str] = None
    version_tag: Optional[str] = None
    image_reference: Optional[str] = None
    status: str  # BuildStatus enum value
    logs: Optional[str] = None
    k8s_job_name: Optional[str] = None
    error_message: Optional[str] = None

class AgentDeploymentStatusUpdateRequest(BaseModel):
    agent_id: str
    build_id: Optional[str] = None
    status: str  # DeploymentStatus enum value
    service_url: Optional[str] = None
    k8s_deployment_name: Optional[str] = None
    namespace: Optional[str] = "nasiko-agents"
    error_message: Optional[str] = None


# User Agents API Types (Registry + Upload Combined)
class UserAgentItemResponse(BaseModel):
    # Core agent info
    id: str  # Agent ID
    name: str
    version: str
    description: str
    url: str

    # Protocol info
    protocolVersion: str = "0.2.9"
    preferredTransport: str = "JSONRPC"

    # Provider info
    provider: Optional[Dict[str, Any]] = None
    iconUrl: Optional[str] = None
    documentationUrl: Optional[str] = None

    # Capabilities and config
    capabilities: Dict[str, Any] = {}
    securitySchemes: Dict[str, Any] = {}
    security: List[Any] = []
    defaultInputModes: List[str] = []
    defaultOutputModes: List[str] = []
    skills: List[Dict[str, Any]] = []
    supportsAuthenticatedExtendedCard: bool = False
    signatures: List[Any] = []
    additionalInterfaces: Optional[List[Dict[str, str]]] = None

    upload_id: Optional[str] = None  # Only for uploaded agents
    created_at: Optional[str] = None
    updated_at: Optional[str] = None


class UserAgentsResponse(BaseModel):
    data: List[UserAgentItemResponse]
    status_code: int
    message: str


# Simplified Response Types
class UploadInfoResponse(BaseModel):
    upload_type: str  # "zip", "directory", "n8n_register", "github_clone"
    upload_status: str  # "Active", "Setting Up", "Failed"


class SimpleUserUploadAgentResponse(BaseModel):
    agent_id: str  # database id field mapped to agent_id
    agent_name: str  # database name field mapped to agent_name
    icon_url: Optional[str] = None
    upload_info: UploadInfoResponse
    tags: List[str] = []
    description: Optional[str] = None


class SimpleUserAgentResponse(BaseModel):
    agent_id: str
    name: str
    icon_url: Optional[str] = None
    tags: List[str] = []
    description: Optional[str] = None


class SimpleUserUploadAgentsResponse(BaseModel):
    data: List[SimpleUserUploadAgentResponse]
    status_code: int
    message: str


class SimpleUserAgentsResponse(BaseModel):
    data: List[SimpleUserAgentResponse]
    status_code: int
    message: str


# Search Response Types
class UserSearchResult(BaseModel):
    id: str
    username: str
    display_name: Optional[str] = None
    email: Optional[str] = None
    role: Optional[str] = None
    avatar_url: Optional[str] = None
    score: Optional[float] = None


class AgentSearchResult(BaseModel):
    agent_id: str  # database id field mapped to agent_id
    agent_name: str  # database name field mapped to agent_name
    description: Optional[str] = None
    tags: List[str] = []
    icon_url: Optional[str] = None
    owner_id: Optional[str] = None
    version: Optional[str] = None
    score: Optional[float] = None


class UserSearchResponse(BaseModel):
    data: List[UserSearchResult]
    query: str
    total_matches: int
    showing: int
    status_code: int
    message: str


class AgentSearchResponse(BaseModel):
    data: List[AgentSearchResult]
    query: str
    total_matches: int
    showing: int
    status_code: int
    message: str


# Agent Update API Types
class AgentVersionInfo(BaseModel):
    version: str
    status: str  # "active", "archived", "failed"
    created_at: str
    build_ids: List[str] = []
    deployment_ids: List[str] = []
    git_commit: Optional[str] = None
    rollback_info: Optional[Dict[str, Any]] = None

class AgentUpdateRequest(BaseModel):
    version: Optional[str] = "auto"  # "auto", "major", "minor", "patch", or specific version
    update_strategy: str = "rolling"  # "rolling" or "blue-green"
    cleanup_old: bool = True
    description: Optional[str] = None  # Update description

class AgentUpdateResponse(BaseModel):
    message: str
    agent_id: str
    new_version: str
    previous_version: Optional[str] = None
    build_id: Optional[str] = None
    deployment_id: Optional[str] = None
    update_strategy: str
    status: str  # "building", "deploying", "completed", "failed"
    status_code: int

class AgentRollbackRequest(BaseModel):
    target_version: Optional[str] = None  # Defaults to previous version
    cleanup_failed: bool = True
    reason: Optional[str] = None

class AgentRollbackResponse(BaseModel):
    message: str
    agent_id: str
    rolled_back_to: str
    rolled_back_from: str
    status: str
    status_code: int

class AgentVersionHistoryResponse(BaseModel):
    agent_id: str
    current_version: str
    versions: List[AgentVersionInfo]
    status_code: int
    message: str

class AgentRebuildRequest(BaseModel):
    reason: Optional[str] = None
    force: bool = False  # Force rebuild even if already building

class AgentRebuildResponse(BaseModel):
    message: str
    agent_id: str
    version: str
    build_id: str
    status: str
    status_code: int


# Version Mapping API Types
class VersionMappingRequest(BaseModel):
    agent_id: str
    semantic_version: str

class VersionMappingResponse(BaseModel):
    agent_id: str
    semantic_version: str
    image_tag: str
    timestamp: int
    status_code: int
    message: str

class VersionStatusUpdateRequest(BaseModel):
    status: str  # "building", "active", "failed", etc.

class VersionStatusUpdateResponse(BaseModel):
    agent_name: str
    status: str
    status_code: int
    message: str


# NANDA API Types
class NANDAAgentFacts(BaseModel):
    username: Optional[str] = None
    _id: Optional[str] = None
    id: Optional[str] = None
    agent_name: Optional[str] = None
    label: Optional[str] = None
    description: Optional[str] = None
    version: Optional[str] = None
    documentationUrl: Optional[str] = None
    jurisdiction: Optional[str] = None
    provider: Optional[Dict[str, Any]] = None
    endpoints: Optional[Dict[str, Any]] = None
    capabilities: Optional[Dict[str, Any]] = None
    skills: Optional[List[Dict[str, Any]]] = None
    evaluations: Optional[Dict[str, Any]] = None
    telemetry: Optional[Dict[str, Any]] = None
    certification: Optional[Dict[str, Any]] = None
    userId: Optional[str] = None
    created_at: Optional[str] = None
    updated_at: Optional[str] = None
    iotMetadata: Optional[Dict[str, Any]] = None


class NANDAAgent(BaseModel):
    id: str
    name: str
    description: str
    endpoint: str
    status: str
    category: str
    factsUrl: Optional[str] = None
    agentFacts: Optional[NANDAAgentFacts] = None
    lastSeen: Optional[str] = None
    messageCount: int = 0
    specialties: List[str] = []
    subCategory: Optional[str] = None


class NANDAPagination(BaseModel):
    page: int
    limit: int
    total: int
    totalPages: int
    hasNext: bool
    hasPrev: bool


class NANDAAgentsResponse(BaseModel):
    agents: List[NANDAAgent]
    pagination: NANDAPagination


class NANDAAgentsListRequest(BaseModel):
    type: Optional[str] = "all"  # "all", "skill", "persona", "communication", "iot"
    limit: Optional[int] = 10
    page: Optional[int] = 1
    status: Optional[str] = None  # "online", "offline"
    category: Optional[str] = None
    search: Optional[str] = None


class NANDAAgentDetailResponse(BaseModel):
    agent: NANDAAgent
    status_code: int = 200
    message: str = "Success"


class NANDAApiResponse(BaseModel):
    success: bool
    data: Optional[Any] = None
    message: str
    status_code: int = 200


# NANDA Messages API Types
class NANDAMessageContent(BaseModel):
    message: Optional[str] = None
    raw_response: Optional[Dict[str, Any]] = None


class NANDAMessage(BaseModel):
    _id: str
    timestamp: str
    type: str  # "a2a_response", "a2a_send"
    from_agent: str
    to_agent: str
    content: Union[str, NANDAMessageContent]
    conversation_id: str
    agent_id: str
    response_to: Optional[str] = None
    from_region: Optional[str] = None
    to_region: Optional[str] = None


class NANDAMessagesResponse(BaseModel):
    messages: List[NANDAMessage]
    total: Optional[int] = None
    has_more: Optional[bool] = None


class NANDAMessagesListRequest(BaseModel):
    limit: Optional[int] = 20
    offset: Optional[int] = None
    before: Optional[str] = None  # Message ID for pagination
    after: Optional[str] = None   # Message ID for pagination
    agent_id: Optional[str] = None  # Filter by specific agent
    conversation_id: Optional[str] = None  # Filter by conversation
    message_type: Optional[str] = None  # Filter by type (a2a_response, a2a_send)
