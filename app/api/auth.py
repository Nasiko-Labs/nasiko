"""
JWT Authentication for App via Auth Service
Validates JWT tokens by calling the auth service
"""

import httpx
import logging
from fastapi import HTTPException, status, Depends, Header
from fastapi.security import HTTPBearer, HTTPAuthorizationCredentials
from typing import Optional
import os

logger = logging.getLogger(__name__)

# Auth service configuration
AUTH_SERVICE_URL = os.getenv("AUTH_SERVICE_URL", "http://nasiko-auth-service:8001")

# Security scheme for Swagger docs
security = HTTPBearer()

class AuthUser:
    """Represents an authenticated user"""
    def __init__(self, user_id: str, subject_type: str = "user"):
        self.user_id = user_id
        self.subject_type = subject_type

async def validate_token_with_auth_service(token: str) -> dict:
    """Validate token by calling auth service /auth/validate endpoint"""
    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            response = await client.post(
                f"{AUTH_SERVICE_URL}/auth/validate",
                headers={
                    "Authorization": f"Bearer {token}",
                    "Content-Type": "application/json"
                }
            )
            
            if response.status_code == 200:
                validation_data = response.json()
                if validation_data.get("valid"):
                    return validation_data
            
            logger.warning(f"Token validation failed: {response.status_code} - {response.text}")
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Invalid or expired token",
                headers={"WWW-Authenticate": "Bearer"},
            )
            
    except httpx.RequestError as e:
        logger.error(f"Failed to connect to auth service: {e}")
        raise HTTPException(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
            detail="Authentication service unavailable"
        )
    except HTTPException:
        raise
    except Exception as e:
        logger.error(f"Token validation error: {e}")
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Token validation failed",
            headers={"WWW-Authenticate": "Bearer"},
        )

async def get_current_user(credentials: HTTPAuthorizationCredentials = Depends(security)) -> AuthUser:
    """
    Dependency to get current authenticated user from JWT token
    Usage: user = Depends(get_current_user)
    """
    token = credentials.credentials
    validation_data = await validate_token_with_auth_service(token)
    
    user_id = validation_data.get("subject_id")
    subject_type = validation_data.get("subject_type", "user")
    
    if not user_id:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Invalid token payload",
            headers={"WWW-Authenticate": "Bearer"},
        )
    
    return AuthUser(user_id=user_id, subject_type=subject_type)

async def get_current_user_optional(
    credentials: Optional[HTTPAuthorizationCredentials] = Depends(HTTPBearer(auto_error=False))
) -> Optional[AuthUser]:
    """
    Optional authentication - returns user if token provided and valid, None otherwise
    Usage: user = Depends(get_current_user_optional)
    """
    if not credentials:
        return None
    
    try:
        token = credentials.credentials
        validation_data = await validate_token_with_auth_service(token)
        
        user_id = validation_data.get("subject_id")
        subject_type = validation_data.get("subject_type", "user")
        
        if not user_id:
            return None
        
        return AuthUser(user_id=user_id, subject_type=subject_type)
    except HTTPException:
        return None

async def get_user_id_from_token(credentials: HTTPAuthorizationCredentials = Depends(security)) -> str:
    """
    Extract user_id from validated JWT token for backward compatibility
    Usage: user_id = Depends(get_user_id_from_token)
    """
    user = await get_current_user(credentials)
    return user.user_id

async def verify_token_header(authorization: str = Header(..., description="Bearer token")) -> str:
    """
    Verify token from Authorization header and return user_id
    Usage: user_id = Depends(verify_token_header)
    """
    if not authorization.startswith("Bearer "):
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Invalid authorization header format. Expected: 'Bearer <token>'",
            headers={"WWW-Authenticate": "Bearer"},
        )
    
    token = authorization.split(" ", 1)[1]
    validation_data = await validate_token_with_auth_service(token)
    
    user_id = validation_data.get("subject_id")
    if not user_id:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Invalid token payload",
            headers={"WWW-Authenticate": "Bearer"},
        )
    
    return user_id

async def get_super_user(credentials: HTTPAuthorizationCredentials = Depends(security)) -> AuthUser:
    """
    Dependency to get current authenticated super user
    Validates user is super user and returns user info
    Usage: user = Depends(get_super_user)
    """
    token = credentials.credentials
    validation_data = await validate_token_with_auth_service(token)
    
    user_id = validation_data.get("subject_id")
    subject_type = validation_data.get("subject_type", "user")
    is_super_user = validation_data.get("is_super_user", False)
    
    if not user_id:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Invalid token payload",
            headers={"WWW-Authenticate": "Bearer"},
        )
    
    # Check if user is super user from token validation response
    if not is_super_user:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Super user access required"
        )
    
    return AuthUser(user_id=user_id, subject_type=subject_type)