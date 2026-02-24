"""
Base handler class with common utilities and shared functionality
"""

from typing import Any, Dict, Optional
import logging

class BaseHandler:
    """
    Base class for all handlers providing common utilities and shared functionality
    """
    
    def __init__(self, service, logger):
        self.service = service
        self.logger = logger
        
    def log_info(self, message: str, **kwargs):
        """Structured logging for info level"""
        if kwargs:
            self.logger.info(f"{message} | {kwargs}")
        else:
            self.logger.info(message)
            
    def log_error(self, message: str, error: Exception = None, **kwargs):
        """Structured logging for error level"""
        if error:
            self.logger.error(f"{message} | Error: {str(error)} | {kwargs}")
        else:
            self.logger.error(f"{message} | {kwargs}")
            
    def log_warning(self, message: str, **kwargs):
        """Structured logging for warning level"""
        if kwargs:
            self.logger.warning(f"{message} | {kwargs}")
        else:
            self.logger.warning(message)
            
    def log_debug(self, message: str, **kwargs):
        """Structured logging for debug level"""
        if kwargs:
            self.logger.debug(f"{message} | {kwargs}")
        else:
            self.logger.debug(message)
            
    async def handle_service_error(self, operation: str, error: Exception):
        """Common error handling for service operations"""
        self.log_error(f"Service operation failed: {operation}", error)
        # Could add more common error handling logic here
        raise error
        
    def validate_required_fields(self, data: Dict[str, Any], required_fields: list) -> bool:
        """Validate that all required fields are present in data"""
        missing_fields = [field for field in required_fields if field not in data or data[field] is None]
        if missing_fields:
            self.log_error(f"Missing required fields: {missing_fields}")
            return False
        return True
        
    def sanitize_string(self, value: str, max_length: int = 255) -> str:
        """Basic string sanitization"""
        if not value:
            return ""
        return str(value).strip()[:max_length]
        
    def build_success_response(self, data: Any, message: str = "Operation successful") -> Dict[str, Any]:
        """Build a standardized success response"""
        return {
            "success": True,
            "message": message,
            "data": data
        }
        
    def build_error_response(self, message: str, error_code: str = None) -> Dict[str, Any]:
        """Build a standardized error response"""
        response = {
            "success": False,
            "message": message
        }
        if error_code:
            response["error_code"] = error_code
        return response