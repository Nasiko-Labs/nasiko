"""
User GitHub Credentials Entity - Models for storing encrypted GitHub tokens per user
"""
from enum import Enum

# Enums
class GitHubCredentialType(str, Enum):
    OAUTH_TOKEN = "oauth_token"
    PERSONAL_ACCESS_TOKEN = "personal_access_token"

class GitHubConnectionStatus(str, Enum):
    ACTIVE = "active"
    INACTIVE = "inactive"
    ERROR = "error"
    TESTING = "testing"