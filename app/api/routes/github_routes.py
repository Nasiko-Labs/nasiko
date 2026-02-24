"""
GitHub Routes - GitHub integration and OAuth endpoints
"""

from fastapi import APIRouter, Query, Request, Depends

from ..auth import get_user_id_from_token
from ..handlers import HandlerFactory
from ..types import (
    GithubRepositoryListResponse,
    GithubCloneRequest,
    AgentUploadResponse,
)


def create_github_routes(handlers: HandlerFactory) -> APIRouter:
    """Create GitHub integration routes"""
    router = APIRouter(tags=["GitHub"])

    # Auth Endpoints
    @router.get(
        "/auth/github/login",
        summary="GitHub Login",
        description="Returns GitHub OAuth authorization URL",
    )
    async def github_login(request: Request, user_id: str = Depends(get_user_id_from_token)):
        return await handlers.github.github_login(user_id, request)

    @router.get(
        "/auth/github/callback",
        summary="GitHub OAuth Callback",
        description=(
            "Shared callback endpoint for GitHub OAuth flows. "
            "Handles both GitHub account connection and user login based on state."
        ),
    )
    async def github_callback(
        request: Request,
        code: str = Query(..., description="OAuth code from GitHub"),
        state: str = Query(..., description="Signed state containing OAuth flow metadata"),
    ):
        return await handlers.github.github_callback(code, state, request)

    @router.get(
        "/auth/github/token",
        summary="Get GitHub Token",
        description="Get current user's GitHub access token status",
    )
    async def get_github_token(user_id: str = Depends(get_user_id_from_token)):
        return await handlers.github.get_github_access_token(user_id)

    @router.post(
        "/auth/github/logout",
        summary="GitHub Logout",
        description="Logout from GitHub - removes stored token for user",
    )
    async def github_logout(user_id: str = Depends(get_user_id_from_token)):
        return await handlers.github.github_logout(user_id)

    # GitHub Repository Endpoints
    @router.get(
        "/github/repositories",
        response_model=GithubRepositoryListResponse,
        summary="List GitHub Repositories",
        description="List user's GitHub repositories (requires authentication)",
    )
    async def list_github_repositories(user_id: str = Depends(get_user_id_from_token)):
        return await handlers.github.list_github_repositories(user_id)

    @router.post(
        "/github/clone",
        response_model=AgentUploadResponse,
        status_code=201,
        summary="Clone GitHub Repository",
        description="Clone a GitHub repository and upload it as an agent",
    )
    async def clone_github_repository(
        clone_request: GithubCloneRequest,
        user_id: str = Depends(get_user_id_from_token),
    ):
        return await handlers.github.clone_github_repository(clone_request, user_id)

    # User Login with GitHub OAuth (Public - No Auth Required)
    @router.get(
        "/auth/github/login-user",
        summary="GitHub User Login",
        description=(
            "Initiate GitHub OAuth for user authentication (not repo cloning). "
            "Uses the shared /auth/github/callback endpoint."
        )
    )
    async def github_user_login(request: Request):
        """
        Public endpoint - no auth required.
        Returns GitHub OAuth URL for user login.
        """
        return await handlers.github.github_user_login(request)

    return router
