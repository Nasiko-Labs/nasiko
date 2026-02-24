"""
Secure API Client for Nasiko CLI.
Handles authenticated requests with automatic token renewal.
"""

import json
from typing import Any, Dict, Optional, Union
import requests
import typer
from requests.adapters import HTTPAdapter
from urllib3.util.retry import Retry

import sys
import os
sys.path.append(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from auth.auth_manager import get_auth_manager
from setup.config import get_cluster_api_url


class APIClient:
    """Authenticated API client for Nasiko services"""
    
    def __init__(self, base_url: str = None, cluster_name: str = None):
        """
        Initialize API client.
        
        Args:
            base_url: Explicit base URL (overrides cluster lookup)
            cluster_name: Name of the cluster to connect to
        """
        
        # Determine Base URL
        self.cluster_name = cluster_name
        
        if base_url:
            self.base_url = base_url.rstrip('/')
        elif cluster_name:
            self.cluster_name = cluster_name
            # Look up URL for specific cluster
            url = get_cluster_api_url(cluster_name)
            if not url:
                typer.echo(f"❌ Unknown cluster: {cluster_name}")
                typer.echo("   Use 'nasiko list-clusters' to see available clusters")
                raise typer.Exit(1)
            self.base_url = url.rstrip('/')
        else:
            # Fallback 1: NASIKO_API_URL env var
            env_url = os.getenv("NASIKO_API_URL")
            if env_url:
                self.base_url = env_url.rstrip('/')
            else:
                # Fallback 2: Check env var for default cluster
                env_cluster = os.environ.get("NASIKO_CLUSTER_NAME")
                if env_cluster:
                    self.cluster_name = env_cluster
                    url = get_cluster_api_url(env_cluster)
                    if url:
                        self.base_url = url.rstrip('/')
                    else:
                        # Env var cluster not found? Fallback to localhost or maybe error?
                        # Current logic was falling back to localhost implicitly.
                        # We'll stick to localhost but maybe without cluster name if url not found.
                        self.base_url = "http://localhost:8000"
                else:
                    # Fallback 3: Default to localhost (dev mode)
                    self.base_url = "http://localhost:8000"
        
        # Set API path
        self.api_url = f"{self.base_url}/api/v1"

        # Initialize Auth Manager with the same Base URL
        # We pass the resolved base_url to ensure consistency
        self.auth_manager = get_auth_manager(base_url=self.base_url, cluster_name=self.cluster_name)
        
        # Configure session with retries
        self.session = requests.Session()
        retry_strategy = Retry(
            total=3,
            status_forcelist=[429, 500, 502, 503, 504],
            allowed_methods=["HEAD", "GET", "OPTIONS"],
            backoff_factor=1
        )
        adapter = HTTPAdapter(max_retries=retry_strategy)
        self.session.mount("http://", adapter)
        self.session.mount("https://", adapter)
    
    def _get_full_url(self, endpoint: str) -> str:
        """Get full URL for endpoint"""
        if endpoint.startswith('http'):
            return endpoint
        
        # All APIEndpoints are relative paths like "/registry"
        # We assume they belong under /api/v1 unless they explicitly start with /auth
        
        endpoint = endpoint.lstrip('/')
        
        if endpoint.startswith('auth/'):
            # Auth service routes (e.g. /auth/users/...) are usually under root
            return f"{self.base_url}/{endpoint}"
        else:
            # Standard API routes
            return f"{self.api_url}/{endpoint}"
    
    def _require_auth(self):
        """Ensure user is authenticated"""
        if not self.auth_manager.is_logged_in():
            typer.echo("❌ Please login first:")
            typer.echo("   nasiko login")
            raise typer.Exit(1)
        
        if not self.auth_manager.refresh_token_if_needed():
            typer.echo("❌ Authentication failed. Please login again:")
            typer.echo("   nasiko login")
            raise typer.Exit(1)
    
    def _make_request(self, method: str, endpoint: str, require_auth: bool = True, **kwargs) -> requests.Response:
        """Make an authenticated API request"""
        
        if require_auth:
            self._require_auth()
            headers = self.auth_manager.get_auth_headers()
            kwargs.setdefault('headers', {}).update(headers or {})
        
        url = self._get_full_url(endpoint)
        
        # Set default headers
        default_headers = {
            'Content-Type': 'application/json',
            'User-Agent': 'Nasiko-CLI/1.0.0'
        }
        kwargs.setdefault('headers', {})
        for key, value in default_headers.items():
            kwargs['headers'].setdefault(key, value)
        
        # Set default timeout
        kwargs.setdefault('timeout', 30)
        
        try:
            response = self.session.request(method, url, **kwargs)
            
            # Handle auth failures
            if response.status_code == 401 and require_auth:
                typer.echo("❌ Authentication failed. Please login again:")
                typer.echo("   nasiko login")
                self.auth_manager.logout()
                raise typer.Exit(1)
            
            return response
            
        except requests.exceptions.RequestException as e:
            typer.echo(f"❌ API request failed: {e}")
            raise typer.Exit(1)
    
    def get(self, endpoint: str, require_auth: bool = True, **kwargs) -> requests.Response:
        """Make GET request"""
        return self._make_request("GET", endpoint, require_auth, **kwargs)
    
    def post(self, endpoint: str, data: Any = None, require_auth: bool = True, **kwargs) -> requests.Response:
        """Make POST request"""
        if data is not None and 'json' not in kwargs and 'data' not in kwargs:
            kwargs['json'] = data
        return self._make_request("POST", endpoint, require_auth, **kwargs)
    
    def put(self, endpoint: str, data: Any = None, require_auth: bool = True, **kwargs) -> requests.Response:
        """Make PUT request"""
        if data is not None and 'json' not in kwargs and 'data' not in kwargs:
            kwargs['json'] = data
        return self._make_request("PUT", endpoint, require_auth, **kwargs)
    
    def patch(self, endpoint: str, data: Any = None, require_auth: bool = True, **kwargs) -> requests.Response:
        """Make PATCH request"""
        if data is not None and 'json' not in kwargs and 'data' not in kwargs:
            kwargs['json'] = data
        return self._make_request("PATCH", endpoint, require_auth, **kwargs)
    
    def delete(self, endpoint: str, require_auth: bool = True, **kwargs) -> requests.Response:
        """Make DELETE request"""
        return self._make_request("DELETE", endpoint, require_auth, **kwargs)
    
    # Convenience methods for common response handling
    
    def get_json(self, endpoint: str, require_auth: bool = True, **kwargs) -> Optional[Dict]:
        """GET request returning JSON data"""
        response = self.get(endpoint, require_auth, **kwargs)
        if response.status_code == 200:
            return response.json()
        return None
    
    def post_json(self, endpoint: str, data: Any = None, require_auth: bool = True, **kwargs) -> Optional[Dict]:
        """POST request returning JSON data"""
        response = self.post(endpoint, data, require_auth, **kwargs)
        if response.status_code in [200, 201]:
            return response.json()
        return None
    
    def upload_file(self, endpoint: str, file_path: str, file_param: str = 'file', 
                   additional_data: Dict = None, require_auth: bool = True) -> requests.Response:
        """Upload file with multipart form data"""
        self._require_auth() if require_auth else None
        
        headers = self.auth_manager.get_auth_headers() if require_auth else {}
        # Don't set Content-Type for multipart uploads - let requests handle it
        if 'Content-Type' in headers:
            del headers['Content-Type']
        
        url = self._get_full_url(endpoint)
        
        try:
            with open(file_path, 'rb') as f:
                files = {file_param: f}
                data = additional_data or {}
                
                response = self.session.post(url, files=files, data=data, headers=headers, timeout=300)
                
                if response.status_code == 401 and require_auth:
                    typer.echo("❌ Authentication failed. Please login again:")
                    typer.echo("   nasiko login")
                    self.auth_manager.logout()
                    raise typer.Exit(1)
                
                return response
                
        except requests.exceptions.RequestException as e:
            typer.echo(f"❌ File upload failed: {e}")
            raise typer.Exit(1)
        except FileNotFoundError:
            typer.echo(f"❌ File not found: {file_path}")
            raise typer.Exit(1)
    
    def handle_response(self, response: requests.Response, 
                       success_message: str = None, 
                       error_prefix: str = "API error") -> Optional[Dict]:
        """Handle common response patterns"""
        try:
            if response.status_code in [200, 201]:
                data = response.json()
                if success_message:
                    typer.echo(f"✅ {success_message}")
                return data
            
            elif response.status_code == 404:
                error_data = response.json() if response.content else {}
                typer.echo(f"❌ Not found: {error_data.get('detail', 'Resource not found')}")
                return None
            
            elif response.status_code == 400:
                error_data = response.json() if response.content else {}
                typer.echo(f"❌ Bad request: {error_data.get('detail', 'Invalid request')}")
                return None
            
            elif response.status_code == 403:
                error_data = response.json() if response.content else {}
                typer.echo(f"❌ Forbidden: {error_data.get('detail', 'Access denied')}")
                return None
            
            elif response.status_code == 422:
                error_data = response.json() if response.content else {}
                typer.echo(f"❌ Validation error: {error_data.get('detail', 'Invalid data')}")
                return None
            
            else:
                error_data = response.json() if response.content else {}
                typer.echo(f"❌ {error_prefix} ({response.status_code}): {error_data.get('detail', 'Unknown error')}")
                return None
                
        except json.JSONDecodeError:
            typer.echo(f"❌ {error_prefix} ({response.status_code}): Invalid response format")
            return None
    
    # Auth service methods
    
    def auth_post(self, endpoint: str, data: Any = None, require_auth: bool = True, **kwargs) -> requests.Response:
        """Make POST request to auth service"""
        return self._make_request("POST", endpoint, require_auth, **kwargs)
    
    def auth_get(self, endpoint: str, require_auth: bool = True, **kwargs) -> requests.Response:
        """Make GET request to auth service"""
        return self._make_request("GET", endpoint, require_auth, **kwargs)
    
    def auth_delete(self, endpoint: str, data: Any = None, require_auth: bool = True, **kwargs) -> requests.Response:
        """Make DELETE request to auth service"""
        if data is not None and 'json' not in kwargs and 'data' not in kwargs:
            kwargs['json'] = data
        return self._make_request("DELETE", endpoint, require_auth, **kwargs)


# Global API client instance
_api_client: Optional[APIClient] = None

def get_api_client(cluster_name: Optional[str] = None) -> APIClient:
    """
    Get the global API client instance, or create a new one with specific cluster config.
    
    Args:
        cluster_name: Optional cluster name to connect to. 
                      If None, checks NASIKO_CLUSTER_NAME env var or defaults.
    """
    global _api_client
    
    # If a specific cluster is requested, we bypass the singleton for safety
    if cluster_name:
        return APIClient(cluster_name=cluster_name)
        
    if _api_client is None:
        # Check if environment specifies a default cluster
        default_cluster = os.environ.get("NASIKO_CLUSTER_NAME")
        _api_client = APIClient(cluster_name=default_cluster)
        
    return _api_client


# Convenience functions for common operations
def require_login():
    """Decorator to require login for CLI commands"""
    def decorator(func):
        def wrapper(*args, **kwargs):
            # Pass cluster_name if available in kwargs, otherwise None
            cluster = kwargs.get('cluster_name') or kwargs.get('cluster')
            client = get_api_client(cluster_name=cluster)
            client._require_auth()
            return func(*args, **kwargs)
        return wrapper
    return decorator
