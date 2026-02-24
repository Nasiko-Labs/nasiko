"""
Orchestration Service
Handles communication with orchestrator through Redis streams
"""

import logging
import redis
from typing import Any
from datetime import datetime, UTC
from app.pkg.config.config import settings


class OrchestrationService:
    """Service for sending orchestration commands via Redis streams"""
    
    def __init__(self, logger: logging.Logger):
        self.logger = logger
        self.redis_client = None
        self.stream_name = "orchestration:commands"
        self.connect()
    
    def connect(self):
        """Connect to Redis"""
        try:
            self.redis_client = redis.Redis(
                host=settings.REDIS_HOST,
                port=settings.REDIS_PORT,
                db=settings.REDIS_DB,
                decode_responses=True,
                socket_connect_timeout=5,
                socket_timeout=5
            )
            # Test connection
            self.redis_client.ping()
            self.logger.info(f"Connected to Redis at {settings.REDIS_HOST}:{settings.REDIS_PORT}")
        except Exception as e:
            self.logger.error(f"Failed to connect to Redis: {e}")
            self.redis_client = None
    
    def is_connected(self) -> bool:
        """Check if Redis connection is active"""
        if not self.redis_client:
            return False
        try:
            self.redis_client.ping()
            return True
        except:
            return False
    
    async def trigger_agent_orchestration(
        self,
        agent_name: str,
        agent_path: str,
        base_url: str | None = None,
        additional_data: dict[str, Any] | None = None
    ) -> bool:
        """
        Send orchestration command for agent deployment
        
        Args:
            agent_name: Name of the agent to orchestrate
            agent_path: Path to agent directory (in shared volume)
            base_url: Base URL for agent service
            additional_data: Any additional data to include
            
        Returns:
            True if message was sent successfully, False otherwise
        """
        if not self.is_connected():
            self.logger.warning("Redis not connected, attempting to reconnect...")
            self.connect()
            if not self.is_connected():
                self.logger.error("Failed to send orchestration command - Redis not available")
                return False
        
        try:
            if not base_url:
                base_url = settings.NASIKO_API_URL

            # Create orchestration message
            message = {
                "command": "deploy_agent",
                "agent_name": agent_name,
                "agent_path": agent_path,
                "base_url": base_url,
                "timestamp": datetime.now(UTC).isoformat(),
                "source": "nasiko-backend"
            }
            
            # Add any additional data
            if additional_data:
                message.update(additional_data)
            
            # Send to Redis stream
            message_id = self.redis_client.xadd(self.stream_name, message)
            
            self.logger.info(
                f"Sent orchestration command for agent '{agent_name}' to Redis stream. "
                f"Message ID: {message_id}"
            )
            
            return True
            
        except Exception as e:
            self.logger.error(f"Failed to send orchestration command for {agent_name}: {e}")
            return False
    
    async def get_agent_status(self, agent_name: str) -> dict[str, Any] | None:
        """
        Get agent deployment status from Redis
        
        Args:
            agent_name: Name of the agent
            
        Returns:
            Agent status dict or None if not found
        """
        if not self.is_connected():
            return None
        
        try:
            status_key = f"agent:status:{agent_name}"
            status_data = self.redis_client.hgetall(status_key)
            
            if status_data:
                # Convert timestamp strings back to proper format
                if 'last_updated' in status_data:
                    status_data['last_updated'] = status_data['last_updated']
                
                return status_data
            
            return None
            
        except Exception as e:
            self.logger.error(f"Failed to get agent status for {agent_name}: {e}")
            return None
    
    async def set_agent_status(
        self, 
        agent_name: str, 
        status: str, 
        details: dict[str, Any] | None = None
    ) -> bool:
        """
        Set agent deployment status in Redis
        
        Args:
            agent_name: Name of the agent
            status: Status string (e.g., 'building', 'running', 'failed')
            details: Additional status details
            
        Returns:
            True if status was set successfully
        """
        if not self.is_connected():
            return False
        
        try:
            status_key = f"agent:status:{agent_name}"
            status_data = {
                "agent_name": agent_name,
                "status": status,
                "last_updated": datetime.now(UTC).isoformat()
            }
            
            if details:
                # Filter out None values that Redis can't store
                filtered_details = {k: v for k, v in details.items() if v is not None}
                status_data.update(filtered_details)
            
            # Store as hash for easy field access
            self.redis_client.hset(status_key, mapping=status_data)
            
            # Set expiration (24 hours)
            self.redis_client.expire(status_key, 86400)
            
            self.logger.debug(f"Set agent status for {agent_name}: {status}")
            return True
            
        except Exception as e:
            self.logger.error(f"Failed to set agent status for {agent_name}: {e}")
            return False
    
    def close(self):
        """Close Redis connection"""
        if self.redis_client:
            self.redis_client.close()
            self.logger.debug("Closed Redis connection")
