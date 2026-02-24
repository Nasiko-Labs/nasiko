"""
GitHub Service - Handles GitHub OAuth, API calls, and credential management
"""

import httpx
import asyncio
import shutil
import tempfile
import os
import base64
import hashlib
import hmac
import json
import secrets
from typing import Any
from datetime import datetime, timezone
from urllib.parse import urlencode

from app.entity.user_github_credentials_entity import GitHubCredentialType, GitHubConnectionStatus


class GitHubService:
    """GitHub service for OAuth, API calls, and repository operations"""

    OAUTH_STATE_VERSION = "v1"
    OAUTH_STATE_MAX_AGE_SECONDS = 600

    def __init__(self, repository, logger, client_id: str = None, client_secret: str = None):
        self.repo = repository
        self.logger = logger

        # Prefer explicitly passed values, then environment variables, then app settings.
        # This keeps behavior consistent with the rest of the backend which uses Pydantic settings.
        app_settings = None
        try:
            from app.pkg.config.config import settings as app_settings  # type: ignore
        except Exception:
            app_settings = None

        self.client_id = (
            client_id
            or os.getenv("GITHUB_CLIENT_ID")
            or (getattr(app_settings, "GITHUB_CLIENT_ID", None) if app_settings else None)
        )
        self.client_secret = (
            client_secret
            or os.getenv("GITHUB_CLIENT_SECRET")
            or (getattr(app_settings, "GITHUB_CLIENT_SECRET", None) if app_settings else None)
        )
        
        if not self.client_id or not self.client_secret:
            self.logger.warning("GitHub OAuth credentials not configured. Some features may not work.")
    
    async def get_github_auth_url(self, user_id: str, request=None) -> str:
        """Generate GitHub OAuth authorization URL using user_id as state"""
        if not self.client_id:
            raise ValueError("GitHub OAuth not configured")
        
        # Extract base URL from request headers
        base_url = self._get_base_url_from_request(request)
        state = self._build_oauth_state(flow="connect", user_id=user_id)
        
        params = {
            'client_id': self.client_id,
            'redirect_uri': self._get_github_callback_url(base_url),
            'scope': 'repo,user:email',  # Request repository and email access
            'state': state,
        }
        
        auth_url = f"https://github.com/login/oauth/authorize?{urlencode(params)}"
        self.logger.info(f"Generated GitHub auth URL for user {user_id} with base URL {base_url}")
        return auth_url

    def resolve_oauth_state(self, state: str) -> dict[str, Any]:
        """
        Resolve OAuth callback state and return flow metadata.

        Returns:
            {"flow": "connect", "user_id": "..."} or {"flow": "login"}
        """
        if not state:
            raise ValueError("Missing OAuth state")

        if state.startswith(f"{self.OAUTH_STATE_VERSION}."):
            parsed = self._decode_oauth_state(state)
            if not parsed:
                raise ValueError("Invalid OAuth state")

            flow = parsed.get("flow")
            if flow == "connect":
                user_id = parsed.get("user_id")
                if not user_id:
                    raise ValueError("Missing user_id in OAuth state")
                return {"flow": "connect", "user_id": user_id}
            if flow == "login":
                return {"flow": "login"}

            raise ValueError("Unknown OAuth flow in signed state")

        # Backward compatibility for legacy non-signed state values
        if state.startswith("connect:"):
            user_id = state.split(":", 1)[1]
            if not user_id:
                raise ValueError("Invalid connect state")
            return {"flow": "connect", "user_id": user_id}
        if state.startswith("login:"):
            return {"flow": "login"}

        # Legacy connect flow used raw user_id as state.
        return {"flow": "connect", "user_id": state}

    async def handle_github_callback(self, code: str, user_id: str) -> dict[str, Any]:
        """Handle GitHub OAuth callback and store user credentials"""
        try:
            # Exchange authorization code for access token
            token_data = await self._exchange_code_for_token(code)
            if not token_data.get('access_token'):
                raise ValueError("Failed to get access token from GitHub")
            
            access_token = token_data['access_token']
            scopes = token_data.get('scope', '').split(',') if token_data.get('scope') else []
            
            # Get user information from GitHub
            user_info = await self._get_github_user_info(access_token)
            if not user_info:
                raise ValueError("Failed to get user information from GitHub")
            
            # Store/update credentials in database
            credential_data = {
                'user_id': user_id,
                'credential_type': GitHubCredentialType.OAUTH_TOKEN,
                'connection_name': f"GitHub - {user_info.get('login', 'Unknown')}",
                'access_token': access_token,  # Will be encrypted by repository
                'github_username': user_info.get('login'),
                'github_user_id': str(user_info.get('id')),
                'avatar_url': user_info.get('avatar_url'),
                'scopes': scopes,
                'connection_status': GitHubConnectionStatus.ACTIVE,
                'last_tested': datetime.now(timezone.utc),
            }
            
            # Use upsert to create or update existing credential
            await self.repo.upsert_user_github_credential(credential_data)
            
            self.logger.info(f"Successfully stored GitHub credentials for user {user_id} ({user_info.get('login')})")
            
            return {
                'success': True,
                'message': 'GitHub authentication successful',
                'user_info': user_info
            }
            
        except Exception as e:
            self.logger.error(f"Error handling GitHub callback for user {user_id}: {e}")
            return {
                'success': False,
                'message': f'GitHub authentication failed: {str(e)}'
            }

    async def get_github_access_token(self, user_id: str) -> dict[str, Any]:
        """Get GitHub access token status for user"""
        try:
            credential = await self.repo.get_user_github_credential_by_user_id(user_id)
            
            if not credential:
                return {
                    'success': False,
                    'message': 'No GitHub credentials found for user',
                    'status': 'not_connected'
                }
            
            # Test the token by making a simple API call
            decrypted_credential = await self.repo.get_user_github_credential_decrypted(user_id)
            if decrypted_credential and decrypted_credential.get('access_token'):
                token_valid = await self._test_github_token(decrypted_credential['access_token'])
                
                if token_valid:
                    # Update connection status to active
                    await self.repo.update_github_credential_test_result(
                        user_id, GitHubConnectionStatus.ACTIVE
                    )
                    
                    return {
                        'success': True,
                        'message': 'GitHub token is valid',
                        'status': 'connected',
                        'username': credential.get('github_username'),
                        'avatar_url': credential.get('avatar_url'),
                        'last_tested': credential.get('last_tested')
                    }
                else:
                    # Update connection status to error
                    await self.repo.update_github_credential_test_result(
                        user_id, GitHubConnectionStatus.ERROR
                    )
                    
                    return {
                        'success': False,
                        'message': 'GitHub token is invalid or expired',
                        'status': 'token_expired'
                    }
            
            return {
                'success': False,
                'message': 'GitHub credentials found but token is invalid',
                'status': 'invalid_credential'
            }
            
        except Exception as e:
            self.logger.error(f"Error getting GitHub token status for user {user_id}: {e}")
            return {
                'success': False,
                'message': f'Error checking GitHub token: {str(e)}',
                'status': 'error'
            }

    async def github_logout(self, user_id: str) -> dict[str, Any]:
        """Remove stored GitHub credentials for user"""
        try:
            deleted = await self.repo.delete_user_github_credential(user_id)
            
            if deleted:
                self.logger.info(f"Successfully removed GitHub credentials for user {user_id}")
                return {
                    'success': True,
                    'message': 'GitHub credentials removed successfully'
                }
            else:
                return {
                    'success': False,
                    'message': 'No GitHub credentials found to remove'
                }
                
        except Exception as e:
            self.logger.error(f"Error removing GitHub credentials for user {user_id}: {e}")
            return {
                'success': False,
                'message': f'Error removing GitHub credentials: {str(e)}'
            }

    async def list_github_repositories(self, user_id: str) -> dict[str, Any]:
        """List user's GitHub repositories using stored token"""
        try:
            # Get user's GitHub credentials
            credential = await self.repo.get_user_github_credential_decrypted(user_id)
            
            if not credential or not credential.get('access_token'):
                raise ValueError("No valid GitHub credentials found")
            
            access_token = credential['access_token']
            
            # Fetch repositories from GitHub
            repositories = await self._fetch_github_repositories(access_token)
            
            self.logger.info(f"Fetched {len(repositories)} repositories for user {user_id}")
            
            return {
                'repositories': repositories,
                'total': len(repositories)
            }
            
        except Exception as e:
            self.logger.error(f"Error listing GitHub repositories for user {user_id}: {e}")
            raise

    async def clone_github_repository(self, clone_request, user_id: str) -> dict[str, Any]:
        """Clone a GitHub repository and upload as agent"""
        try:
            # Get user's GitHub credentials
            credential = await self.repo.get_user_github_credential_decrypted(user_id)
            
            if not credential or not credential.get('access_token'):
                raise ValueError("No valid GitHub credentials found")
            
            access_token = credential['access_token']
            
            # Clone repository to temporary directory
            temp_dir = await self._clone_repository(
                clone_request.repository_full_name,
                clone_request.branch or 'main',
                access_token
            )
            
            # Process the cloned repository as an agent upload
            # This would integrate with the existing agent upload service
            from app.service.agent_upload_tracking_service import AgentUploadTrackingService
            upload_service = AgentUploadTrackingService(self.logger, self.repo)
            
            # Use GitHub upload processing with metadata tracking
            result = await upload_service.process_github_upload(
                temp_dir,
                user_id,
                clone_request.agent_name,
                clone_request.repository_full_name,
                clone_request.branch or 'main'
            )
            
            # Clean up temporary directory
            if temp_dir and os.path.exists(temp_dir):
                shutil.rmtree(temp_dir)
                self.logger.debug(f"Cleaned up temporary directory: {temp_dir}")
            
            return result
            
        except Exception as e:
            self.logger.error(f"Error cloning GitHub repository for user {user_id}: {e}")
            raise

    async def _exchange_code_for_token(self, code: str) -> dict[str, Any]:
        """Exchange OAuth authorization code for access token"""
        token_url = "https://github.com/login/oauth/access_token"
        
        data = {
            'client_id': self.client_id,
            'client_secret': self.client_secret,
            'code': code,
        }
        
        headers = {
            'Accept': 'application/json',
            'Content-Type': 'application/x-www-form-urlencoded'
        }
        
        async with httpx.AsyncClient(timeout=30.0) as client:
            response = await client.post(token_url, data=data, headers=headers)
            
            if response.status_code == 200:
                return response.json()
            else:
                self.logger.error(f"GitHub token exchange failed: {response.status_code} - {response.text}")
                raise ValueError(f"Token exchange failed: {response.status_code}")

    async def _get_github_user_info(self, access_token: str) -> dict[str, Any]:
        """Get GitHub user information using access token"""
        url = "https://api.github.com/user"
        
        headers = {
            'Authorization': f'Bearer {access_token}',
            'Accept': 'application/vnd.github.v3+json',
            'User-Agent': 'Nasiko-Agent-Platform'
        }
        
        async with httpx.AsyncClient(timeout=30.0) as client:
            response = await client.get(url, headers=headers)
            
            if response.status_code == 200:
                return response.json()
            else:
                self.logger.error(f"GitHub user info request failed: {response.status_code} - {response.text}")
                raise ValueError(f"Failed to get user info: {response.status_code}")

    async def _test_github_token(self, access_token: str) -> bool:
        """Test if GitHub access token is valid"""
        try:
            url = "https://api.github.com/user"
            
            headers = {
                'Authorization': f'Bearer {access_token}',
                'Accept': 'application/vnd.github.v3+json',
                'User-Agent': 'Nasiko-Agent-Platform'
            }
            
            async with httpx.AsyncClient(timeout=10.0) as client:
                response = await client.get(url, headers=headers)
                return response.status_code == 200
                
        except Exception:
            return False

    async def _fetch_github_repositories(self, access_token: str) -> list[dict[str, Any]]:
        """Fetch user's GitHub repositories"""
        url = "https://api.github.com/user/repos"
        
        headers = {
            'Authorization': f'Bearer {access_token}',
            'Accept': 'application/vnd.github.v3+json',
            'User-Agent': 'Nasiko-Agent-Platform'
        }
        
        params = {
            'sort': 'updated',
            'direction': 'desc',
            'per_page': 100  # Fetch up to 100 repositories
        }
        
        async with httpx.AsyncClient(timeout=60.0) as client:
            response = await client.get(url, headers=headers, params=params)
            
            if response.status_code == 200:
                repos = response.json()
                
                # Transform the repository data to match our expected format
                transformed_repos = []
                for repo in repos:
                    transformed_repo = {
                        'id': repo['id'],
                        'name': repo['name'],
                        'full_name': repo['full_name'],
                        'description': repo.get('description'),
                        'private': repo['private'],
                        'clone_url': repo['clone_url'],
                        'ssh_url': repo['ssh_url'],
                        'html_url': repo['html_url'],
                        'default_branch': repo.get('default_branch', 'main'),
                        'updated_at': repo['updated_at']
                    }
                    transformed_repos.append(transformed_repo)
                
                return transformed_repos
            else:
                self.logger.error(f"GitHub repositories request failed: {response.status_code} - {response.text}")
                raise ValueError(f"Failed to fetch repositories: {response.status_code}")

    async def _clone_repository(self, repo_full_name: str, branch: str, access_token: str) -> str:
        """Clone GitHub repository to temporary directory"""
        temp_dir = tempfile.mkdtemp(prefix="github_clone_")
        
        try:
            # Use git clone with access token for authentication
            clone_url = f"https://{access_token}@github.com/{repo_full_name}.git"
            
            # Build git clone command
            cmd = ["git", "clone", "--depth", "1", "--branch", branch, clone_url, temp_dir]
            
            # Execute git clone
            process = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE
            )
            
            stdout, stderr = await process.communicate()
            
            if process.returncode == 0:
                self.logger.info(f"Successfully cloned {repo_full_name} (branch: {branch})")
                
                # Remove .git directory to clean up
                git_dir = os.path.join(temp_dir, '.git')
                if os.path.exists(git_dir):
                    shutil.rmtree(git_dir)
                
                return temp_dir
            else:
                error_msg = stderr.decode() if stderr else stdout.decode()
                self.logger.error(f"Git clone failed for {repo_full_name}: {error_msg}")
                
                # Clean up on error
                if os.path.exists(temp_dir):
                    shutil.rmtree(temp_dir)
                
                raise ValueError(f"Failed to clone repository: {error_msg}")
                
        except Exception as e:
            # Clean up on error
            if os.path.exists(temp_dir):
                shutil.rmtree(temp_dir)
            raise

    async def authenticate_with_github_oauth(self, code: str) -> dict[str, Any]:
        """
        Complete GitHub OAuth flow for USER LOGIN (not repo cloning).

        Steps:
        1. Exchange code for GitHub access token
        2. Get GitHub user info from GitHub API
        3. Call auth-service to authenticate/create user
        4. Store GitHub credentials for repo cloning
        5. Return JWT token and user info
        """
        try:
            # 1. Exchange code for access token
            token_data = await self._exchange_code_for_token(code)
            access_token = token_data['access_token']

            # 2. Get GitHub user info
            github_user_info = await self._get_github_user_info(access_token)

            # 3. Handle private email
            email = github_user_info.get('email')
            if not email or email == '':
                # Use placeholder for private emails
                email = f"{github_user_info['login']}@github.user"
                self.logger.info(f"GitHub user {github_user_info['login']} has private email, using placeholder")

            # 4. Call auth-service to authenticate/create user
            auth_service_url = os.getenv("AUTH_SERVICE_URL", "http://nasiko-auth-service:8001")

            async with httpx.AsyncClient(timeout=10.0) as client:
                response = await client.post(
                    f"{auth_service_url}/auth/github/authenticate",
                    json={
                        "github_id": str(github_user_info['id']),
                        "github_username": github_user_info['login'],
                        "email": email,
                        "avatar_url": github_user_info.get('avatar_url')
                    }
                )

                if response.status_code != 200:
                    raise ValueError(f"Auth service returned {response.status_code}: {response.text}")

                auth_data = response.json()
                user_id = auth_data['user_id']

            # 5. Store GitHub credentials for repo cloning (reuse existing logic)
            await self._store_github_credentials_for_repos(
                user_id=user_id,
                access_token=access_token,
                github_info=github_user_info
            )

            return {
                'success': True,
                'token': auth_data['token'],
                'user_id': user_id,
                'is_new_user': auth_data.get('is_new_user', False),
                'is_super_user': auth_data.get('is_super_user', False),
                'github_username': github_user_info['login']
            }

        except Exception as e:
            self.logger.error(f"GitHub OAuth authentication failed: {e}")
            return {
                'success': False,
                'message': f'GitHub authentication failed: {str(e)}'
            }

    async def _store_github_credentials_for_repos(
        self,
        user_id: str,
        access_token: str,
        github_info: dict
    ):
        """Store GitHub credentials for repository cloning (existing functionality)"""
        credential_data = {
            'user_id': user_id,
            'credential_type': GitHubCredentialType.OAUTH_TOKEN,
            'connection_name': f"GitHub - {github_info.get('login', 'Unknown')}",
            'access_token': access_token,
            'github_username': github_info.get('login'),
            'github_user_id': str(github_info.get('id')),
            'avatar_url': github_info.get('avatar_url'),
            'scopes': ['repo', 'user:email'],
            'connection_status': GitHubConnectionStatus.ACTIVE,
            'last_tested': datetime.now(timezone.utc),
        }
        await self.repo.upsert_user_github_credential(credential_data)

    async def get_github_auth_url_for_login(self, request=None) -> str:
        """Generate GitHub OAuth URL for user login (not repo cloning)"""
        if not self.client_id:
            raise ValueError("GitHub OAuth not configured")

        # Extract base URL from request
        base_url = self._get_base_url_from_request(request)
        state = self._build_oauth_state(flow="login")

        params = {
            'client_id': self.client_id,
            'redirect_uri': self._get_github_callback_url(base_url),
            'scope': 'user:email',  # Only need email for login
            'state': state,
        }

        auth_url = f"https://github.com/login/oauth/authorize?{urlencode(params)}"
        self.logger.info(f"Generated GitHub login URL with state {state}")
        return auth_url

    def _get_github_callback_url(self, base_url: str) -> str:
        return f"{base_url}/api/v1/auth/github/callback"

    def _build_oauth_state(self, flow: str, user_id: str | None = None) -> str:
        payload: dict[str, Any] = {
            "flow": flow,
            "iat": int(datetime.now(timezone.utc).timestamp()),
            "nonce": secrets.token_urlsafe(16),
        }
        if user_id:
            payload["user_id"] = user_id

        serialized_payload = json.dumps(payload, separators=(",", ":"), sort_keys=True)
        encoded_payload = base64.urlsafe_b64encode(serialized_payload.encode()).decode().rstrip("=")

        state_secret = self._get_oauth_state_secret()
        if not state_secret:
            if flow == "connect" and user_id:
                self.logger.warning("OAuth state signing secret missing, using fallback connect state format")
                return f"connect:{user_id}"
            self.logger.warning("OAuth state signing secret missing, using fallback login state format")
            return f"login:{payload['nonce']}"

        signature = hmac.new(
            key=state_secret.encode(),
            msg=encoded_payload.encode(),
            digestmod=hashlib.sha256,
        ).hexdigest()
        return f"{self.OAUTH_STATE_VERSION}.{encoded_payload}.{signature}"

    def _decode_oauth_state(self, state: str) -> dict[str, Any] | None:
        try:
            version, encoded_payload, signature = state.split(".", 2)
            if version != self.OAUTH_STATE_VERSION:
                return None
        except ValueError:
            return None

        state_secret = self._get_oauth_state_secret()
        if not state_secret:
            self.logger.warning("OAuth state signing secret missing while decoding state")
            return None

        expected_signature = hmac.new(
            key=state_secret.encode(),
            msg=encoded_payload.encode(),
            digestmod=hashlib.sha256,
        ).hexdigest()
        if not hmac.compare_digest(signature, expected_signature):
            raise ValueError("Invalid OAuth state signature")

        padded_payload = encoded_payload + "=" * (-len(encoded_payload) % 4)
        payload = json.loads(base64.urlsafe_b64decode(padded_payload.encode()).decode())
        issued_at = int(payload.get("iat", 0))
        now = int(datetime.now(timezone.utc).timestamp())
        if not issued_at or now - issued_at > self.OAUTH_STATE_MAX_AGE_SECONDS:
            raise ValueError("OAuth state has expired")

        return payload

    def _get_oauth_state_secret(self) -> str | None:
        return (
            os.getenv("OAUTH_STATE_SIGNING_KEY")
            or self.client_secret
            or os.getenv("SESSION_SECRET_KEY")
        )

    def _get_base_url_from_request(self, request) -> str:
        if not request:
            base = os.getenv("BASE_URL", "http://localhost:8000")
            self.logger.debug(f"[BASE_URL] No request, using fallback: {base}")
            return base

        self.logger.info(f"Request headers: {request.headers}")

        host = request.headers.get("x-forwarded-host") or request.headers.get("host")

        # 1. Prefer Cloudflare (might change later)
        proto = None
        cf_visitor = request.headers.get("cf-visitor")
        if cf_visitor:
            try:
                proto = json.loads(cf_visitor).get("scheme")
                self.logger.debug(f"[BASE_URL] Proto from cf-visitor: {proto}")
            except Exception as e:
                self.logger.debug(f"[BASE_URL] Failed to parse cf-visitor: {e}")

        # 2. Fallback to X-Forwarded-Proto
        if not proto:
            proto = request.headers.get("x-forwarded-proto")
            self.logger.debug(f"[BASE_URL] Proto from x-forwarded-proto: {proto}")

        # 3. Fallback to framework detection
        if not proto:
            proto = request.url.scheme or "http"
            self.logger.debug(f"[BASE_URL] Proto from request.url.scheme: {proto}")

        self.logger.info(f"[BASE_URL] Final resolved host={host}, proto={proto}")

        if host:
            if host == "localhost":
                base = os.getenv("BASE_URL", "http://localhost:8000")
                self.logger.debug(f"[BASE_URL] Localhost detected, using fallback: {base}")
                return base

            base = f"{proto}://{host}"
            self.logger.info(f"[BASE_URL] Returning base URL: {base}")
            return base

        base = os.getenv("BASE_URL", "http://localhost:8000")
        self.logger.debug(f"[BASE_URL] No host found, using fallback: {base}")
        return base
