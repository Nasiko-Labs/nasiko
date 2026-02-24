"""
Configuration settings for Nasiko CLI.
"""

from pathlib import Path

# --- Configuration ---
CONFIG_DIR = Path.home() / ".nasiko"
TOKEN_FILE = CONFIG_DIR / "token"
AGENTS_DIR = Path(__file__).parent.parent / "agents"


# --- API Endpoints ---
class APIEndpoints:
    """Centralized API endpoint definitions (Relative Paths)"""

    # Registry endpoints
    REGISTRY = "/registry"
    REGISTRY_BY_ID = "/registry/{registry_id}"
    REGISTRY_BY_AGENT_NAME = "/registry/agent/name/{agent_name}"
    REGISTRY_BY_AGENT_ID = "/registry/agent/id/{agent_id}"
    REGISTRY_ALL_AGENTS = "/registry/user/agents"

    # Agent upload endpoints
    AGENT_UPLOAD = "/agents/upload"

    # Upload status endpoints
    UPLOAD_STATUS_BY_AGENT = "/upload-status/agent/{agent_name}"
    UPLOAD_STATUS_BY_STATUS = "/upload-status/status/{status}"

    # Observability endpoints
    OBSERVABILITY_SESSIONS = "/observability/session/list"
    OBSERVABILITY_SESSION_DETAILS = "/observability/session/{session_id}"
    OBSERVABILITY_TRACE_DETAILS = "/observability/trace/{project_id}/{trace_id}"
    OBSERVABILITY_SPAN_DETAILS = "/observability/span/{span_id}"
    OBSERVABILITY_AGENT_STATS = "/observability/agent/{agent_id}/stats"

    # GitHub auth endpoints
    GITHUB = "/auth/github"
    GITHUB_LOGIN = "/auth/github/login"
    GITHUB_TOKEN = "/auth/github/token"
    GITHUB_LOGOUT = "/auth/github/logout"
    GITHUB_CALLBACK = "/auth/github/callback"
    GITHUB_REPOSITORY = "/github/repositories"
    GITHUB_CLONE = "/github/clone"

    # Health check
    HEALTHCHECK = "/healthcheck"

    # N8N Integration endpoints
    N8N_REGISTER = "/agents/n8n/register"
    N8N_CONNECT = "/agents/n8n/connect"
    N8N_CREDENTIALS = "/agents/n8n/credentials"
    N8N_WORKFLOWS = "/agents/n8n/workflows"

    # Chat History/Session endpoints
    CHAT_SESSION = "/chat/session"
    CHAT_SESSION_LIST = "/chat/session/list"
    CHAT_SESSION_BY_ID = "/chat/session/{session_id}"

    # Search endpoints
    SEARCH_USERS = "/search/users"
    SEARCH_AGENTS = "/search/agents"
    SEARCH_INDEX_USER = "/search/index/user"

    # Chat Tracking endpoints
    CHAT_TRACKS = "/chat-tracks"
    CHAT_TRACK_BY_ID = "/chat-tracks/{chat_track_id}"
    CHAT_TRACKS_BY_AGENT = "/chat-tracks/agent/{agent_name}"

    # Agent Access Management endpoints (auth service)
    # Note: These are routed via Gateway, so relative path is fine if Gateway handles routing
    AGENT_ACCESS = "/auth/agents"
    AGENT_ACCESS_USERS = "/auth/agents/{agent_id}/access/users"
    AGENT_ACCESS_AGENTS = "/auth/agents/{agent_id}/access/agents"
    AGENT_ACCESS_LIST = "/auth/agents/{agent_id}/access"

    # User Management endpoints (auth service)
    USER_REGISTER = "/auth/users/register"
    USER_LIST = "/auth/users"
    USER_GET = "/auth/users/{user_id}"
    USER_UPDATE = "/auth/users/{user_id}"
    USER_DELETE = "/auth/users/{user_id}"
    USER_REGENERATE_CREDENTIALS = "/auth/users/{user_id}/regenerate-credentials"
    USER_REINSTATE = "/auth/users/{user_id}/reinstate"
    USER_REVOKE_TOKENS = "/auth/tokens/revoke-user/{user_id}"
    EMERGENCY_REVOKE_ALL = "/auth/emergency/revoke-all"
    
    # User Upload Agents endpoint
    USER_UPLOAD_AGENTS = "/user/upload-agents"


# Create config directory if it doesn't exist
CONFIG_DIR.mkdir(parents=True, exist_ok=True)