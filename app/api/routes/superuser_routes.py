"""
Super User Routes - User registration and management endpoints (Super User Only)
"""

from fastapi import APIRouter, HTTPException, Depends
from ..handlers import HandlerFactory
from ..types import (
    UserRegistrationRequest, UserRegistrationResponse
)
from ..auth import get_super_user
import httpx
import os
import logging

logger = logging.getLogger(__name__)
AUTH_SERVICE_URL = os.getenv("AUTH_SERVICE_URL", "http://nasiko-auth-service:8001")

def create_superuser_routes(handlers: HandlerFactory) -> APIRouter:
    """Create super user management routes"""
    router = APIRouter(tags=["Super User Management"])

    @router.post(
        "/user/register",
        response_model=UserRegistrationResponse,
        summary="Register New User",
        description="Register a new user in the system (Super User Only)"
    )
    async def register_user(
        user_data: UserRegistrationRequest,
        current_user = Depends(get_super_user)
    ):
        """Register a new user - only accessible by super users"""
        
        try:
            # Call auth service to register user
            async with httpx.AsyncClient(timeout=30.0) as client:
                registration_payload = {
                    "username": user_data.username,
                    "email": user_data.email,
                    "is_super_user": user_data.is_super_user
                }
                
                response = await client.post(
                    f"{AUTH_SERVICE_URL}/auth/users/register",
                    json=registration_payload
                )
                
                if response.status_code == 200:
                    auth_response = response.json()
                    
                    logger.info(f"User {user_data.username} registered successfully by {current_user.user_id}")
                    
                    return UserRegistrationResponse(
                        user_id=auth_response["user_id"],
                        username=user_data.username,
                        email=user_data.email,
                        role="Super User" if user_data.is_super_user else "User",
                        status="Active",  # New users are always active
                        access_key=auth_response["access_key"],
                        access_secret=auth_response["access_secret"],
                        created_on=auth_response.get("created_at", ""),
                        message="User registered successfully. Store credentials securely - access_secret won't be shown again"
                    )
                else:
                    error_detail = response.text
                    logger.error(f"Auth service registration failed: {error_detail}")
                    raise HTTPException(
                        status_code=response.status_code,
                        detail=f"User registration failed: {error_detail}"
                    )
                    
        except httpx.RequestError as e:
            logger.error(f"Failed to connect to auth service: {e}")
            raise HTTPException(
                status_code=503,
                detail="Authentication service unavailable"
            )
        except HTTPException:
            raise
        except Exception as e:
            logger.error(f"User registration error: {e}")
            raise HTTPException(
                status_code=500,
                detail=f"User registration failed: {str(e)}"
            )

    return router