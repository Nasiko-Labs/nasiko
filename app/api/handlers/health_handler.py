"""
Health Handler - Manages health check operations
"""

from .base_handler import BaseHandler

class HealthHandler:
    """Handler for health check operations"""
    
    def __init__(self):
        # Health handler doesn't need service dependencies
        pass
    
    async def healthcheck(self):
        """Basic health check endpoint"""
        return {"status": "ok"}