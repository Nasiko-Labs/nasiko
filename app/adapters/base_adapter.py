"""
Base adapter interface for external API integrations
Follows the adapter pattern for consistent external API interaction
"""

from abc import ABC, abstractmethod
from typing import Any, Dict, Optional, List
import logging
import httpx
from app.api.types import NANDAApiResponse


class BaseAdapter(ABC):
    """
    Abstract base class for all external API adapters
    Provides common functionality and enforces consistent interface
    """
    
    def __init__(self, base_url: str, timeout: int = 30, logger: Optional[logging.Logger] = None):
        self.base_url = base_url.rstrip('/')
        self.timeout = timeout
        self.logger = logger or logging.getLogger(__name__)
        self._client: Optional[httpx.AsyncClient] = None
    
    async def _get_client(self) -> httpx.AsyncClient:
        """Get or create HTTP client with proper configuration"""
        if self._client is None:
            self._client = httpx.AsyncClient(
                timeout=self.timeout,
                headers=self._get_default_headers()
            )
        return self._client
    
    def _get_default_headers(self) -> Dict[str, str]:
        """Get default headers for all requests"""
        return {
            "Content-Type": "application/json",
            "User-Agent": "NASIKO-API-Client/1.0"
        }
    
    async def _make_request(
        self, 
        method: str, 
        endpoint: str, 
        params: Optional[Dict[str, Any]] = None,
        data: Optional[Dict[str, Any]] = None,
        headers: Optional[Dict[str, str]] = None
    ) -> httpx.Response:
        """
        Make HTTP request with proper error handling and logging
        """
        client = await self._get_client()
        url = f"{self.base_url}{endpoint}"
        
        # Merge headers
        request_headers = self._get_default_headers()
        if headers:
            request_headers.update(headers)
        
        try:
            self.logger.info(f"Making {method} request to {url}", extra={
                "method": method,
                "url": url,
                "params": params
            })
            
            response = await client.request(
                method=method,
                url=url,
                params=params,
                json=data,
                headers=request_headers
            )
            
            self.logger.info(f"Request completed with status {response.status_code}", extra={
                "status_code": response.status_code,
                "url": url
            })
            
            return response
            
        except httpx.TimeoutException as e:
            self.logger.error(f"Request timeout for {url}", extra={"error": str(e)})
            raise
        except httpx.RequestError as e:
            self.logger.error(f"Request error for {url}", extra={"error": str(e)})
            raise
        except Exception as e:
            self.logger.error(f"Unexpected error for {url}", extra={"error": str(e)})
            raise
    
    def _handle_response_error(self, response: httpx.Response) -> NANDAApiResponse:
        """Handle HTTP error responses consistently"""
        try:
            error_data = response.json()
            message = error_data.get('message', f'HTTP {response.status_code} error')
        except:
            message = f'HTTP {response.status_code} error'
        
        return NANDAApiResponse(
            success=False,
            data=None,
            message=message,
            status_code=response.status_code
        )
    
    def _sanitize_unicode(self, obj: Any) -> Any:
        """Recursively sanitize Unicode surrogate characters from data structures"""
        import json
        if isinstance(obj, str):
            # Replace surrogate characters with replacement character
            return obj.encode('utf-8', errors='replace').decode('utf-8')
        elif isinstance(obj, dict):
            return {k: self._sanitize_unicode(v) for k, v in obj.items()}
        elif isinstance(obj, list):
            return [self._sanitize_unicode(item) for item in obj]
        else:
            return obj
    
    def _build_success_response(self, data: Any, message: str = "Success") -> NANDAApiResponse:
        """Build standardized success response with Unicode sanitization"""
        # Sanitize Unicode surrogate characters to prevent encoding errors
        sanitized_data = self._sanitize_unicode(data)
        
        return NANDAApiResponse(
            success=True,
            data=sanitized_data,
            message=message,
            status_code=200
        )
    
    async def close(self):
        """Close HTTP client connection"""
        if self._client:
            await self._client.aclose()
            self._client = None
    
    @abstractmethod
    async def health_check(self) -> NANDAApiResponse:
        """Check if the external API is healthy"""
        pass