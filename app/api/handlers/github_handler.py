"""
GitHub Handler - Manages GitHub integration operations
"""

from fastapi import HTTPException, status, Request
from fastapi.responses import HTMLResponse
from .base_handler import BaseHandler
from ..types import (
    GithubRepositoryListResponse,
    GithubCloneRequest, AgentUploadResponse
)
from app.service.github_service import GitHubService

class GitHubHandler(BaseHandler):
    """Handler for GitHub integration operations"""

    def __init__(self, service, logger):
        super().__init__(service, logger)
        self.github_service = GitHubService(service.repo, logger)

    async def github_login(self, user_id: str, request: Request):
        """Returns the GitHub OAuth authorization URL"""
        try:
            self.log_info("Initiating GitHub OAuth login", user_id=user_id)
            auth_url = await self.github_service.get_github_auth_url(user_id, request)
            return {"auth_url": auth_url}
        except ValueError as e:
            # Common misconfig: missing GITHUB_CLIENT_ID. Don't surface this as a 500.
            raise HTTPException(status_code=status.HTTP_503_SERVICE_UNAVAILABLE, detail=str(e))
        except Exception as e:
            await self.handle_service_error("github_login", e)

    async def github_callback(self, code: str, state: str, request: Request):
        """Shared callback endpoint for both GitHub connect and login flows"""
        try:
            callback_state = self.github_service.resolve_oauth_state(state)
            flow = callback_state["flow"]
            self.log_info("Processing GitHub OAuth callback", flow=flow)

            if flow == "connect":
                user_id = callback_state["user_id"]
                result = await self.github_service.handle_github_callback(code, user_id)
                if result.get('success'):
                    content = "<html><body><h1>GitHub authentication successful!</h1><p>You can now close this window.</p></body></html>"
                else:
                    content = (
                        "<html><body><h1>GitHub authentication failed</h1>"
                        f"<p>{result.get('message', 'Unknown error')}</p></body></html>"
                    )
                return HTMLResponse(content=content)

            if flow == "login":
                result = await self.github_service.authenticate_with_github_oauth(code)
                if result.get('success'):
                    return {
                        "token": result['token'],
                        "token_type": "bearer",
                        "username": result.get('github_username', 'User'),
                        "is_new_user": result.get('is_new_user', False),
                        "is_super_user": result.get('is_super_user', False),
                    }
                raise HTTPException(
                    status_code=status.HTTP_400_BAD_REQUEST,
                    detail=result.get('message', 'GitHub authentication failed')
                )

            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=f"Unsupported OAuth flow: {flow}"
            )
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("github_callback", e)

    async def get_github_access_token(self, user_id: str):
        """Get current token status for user"""
        try:
            self.log_debug("Getting GitHub access token status", user_id=user_id)
            # Get token status from database for user
            result = await self.github_service.get_github_access_token(user_id)
            
            # Map service response to proper HTTP status codes
            status_code = result.get('status', 'unknown')
            success = result.get('success', False)
            
            if success and status_code == 'connected':
                # User is connected, return successful response
                return result
            elif status_code == 'not_connected':
                # User not connected - return 202 (pending) to avoid interceptor logout
                raise HTTPException(
                    status_code=status.HTTP_202_ACCEPTED,
                    detail=result.get('message', 'Authentication pending. Please login with GitHub first.')
                )
            elif status_code == 'token_expired':
                # Token expired - return 202 (pending) to avoid interceptor logout
                raise HTTPException(
                    status_code=status.HTTP_202_ACCEPTED,
                    detail=result.get('message', 'GitHub token has expired. Please re-authenticate.')
                )
            elif status_code == 'invalid_credential':
                # Invalid credentials - return 202 (pending) to avoid interceptor logout
                raise HTTPException(
                    status_code=status.HTTP_202_ACCEPTED,
                    detail=result.get('message', 'GitHub credentials are invalid. Please re-authenticate.')
                )
            elif status_code == 'error':
                # General error - return 500
                raise HTTPException(
                    status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                    detail=result.get('message', 'Internal error while checking GitHub credentials.')
                )
            else:
                # Unknown status - return 500
                raise HTTPException(
                    status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                    detail=f'Unknown GitHub credential status: {status_code}'
                )
                
        except HTTPException:
            # Re-raise HTTP exceptions as-is
            raise
        except Exception as e:
            await self.handle_service_error("get_github_access_token", e)

    async def github_logout(self, user_id: str):
        """Logout from GitHub - removes stored token for user"""
        try:
            self.log_info("Processing GitHub logout", user_id=user_id)
            # Remove stored token from database for user
            result = await self.github_service.github_logout(user_id)
            return result
        except Exception as e:
            await self.handle_service_error("github_logout", e)

    async def list_github_repositories(self, user_id: str) -> GithubRepositoryListResponse:
        """List user's GitHub repositories"""
        try:
            self.log_info("Fetching GitHub repositories", user_id=user_id)
            # Use stored token from database to fetch repositories
            repositories_data = await self.github_service.list_github_repositories(user_id)
            return GithubRepositoryListResponse(**repositories_data)
        except Exception as e:
            await self.handle_service_error("list_github_repositories", e)

    async def clone_github_repository(self, clone_request: GithubCloneRequest, user_id: str) -> AgentUploadResponse:
        """Clone a GitHub repository and upload it as an agent"""
        try:
            self.log_info("Cloning GitHub repository", repository=clone_request.repository_full_name, user_id=user_id)
            # Use stored token from database to clone repository
            result = await self.github_service.clone_github_repository(clone_request, user_id)

            # Convert the result to the expected format
            agent_upload_data = {
                'success': result.success,
                'agent_name': result.agent_name,
                'status': result.status,
                'capabilities_generated': result.capabilities_generated,
                'orchestration_triggered': result.orchestration_triggered,
                'validation_errors': result.validation_errors
            }

            return AgentUploadResponse(
                data=agent_upload_data,
                status_code=201 if result.success else 400,
                message="GitHub repository cloned and uploaded successfully" if result.success else f"Upload failed: {result.status}"
            )
        except Exception as e:
            await self.handle_service_error("clone_github_repository", e)

    async def github_user_login(self, request: Request):
        """Initiate GitHub OAuth for user authentication"""
        try:
            self.log_info("Initiating GitHub OAuth for user login")
            auth_url = await self.github_service.get_github_auth_url_for_login(request)
            return {"auth_url": auth_url}
        except ValueError as e:
            raise HTTPException(status_code=status.HTTP_503_SERVICE_UNAVAILABLE, detail=str(e))
        except Exception as e:
            await self.handle_service_error("github_user_login", e)
