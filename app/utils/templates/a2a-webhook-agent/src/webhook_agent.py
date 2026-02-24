"""
Webhook Agent - Handles all webhook communication logic
"""
import json
import logging
from typing import Dict, Any
import httpx


logger = logging.getLogger(__name__)


class WebhookAgent:
    """
    Handles all webhook communication and response processing
    """
    
    def __init__(self, webhook_url: str, webhook_timeout: int = 120):
        self.webhook_url = webhook_url
        self.webhook_timeout = webhook_timeout
        logger.info(f"WebhookAgent initialized with URL: {webhook_url}")
    
    async def send_message(self, session_id: str, message: str) -> str:
        """
        Send message to webhook and return processed response
        
        Args:
            session_id: The session ID to send (typically the A2A request ID)
            message: The user message text
            
        Returns:
            Processed response text from webhook
        """
        try:
            # Prepare webhook payload
            webhook_payload = {
                "sessionId": session_id,
                "chatInput": message
            }
            
            logger.info(f"Sending to webhook: {self.webhook_url}")
            logger.info(f"Payload: {webhook_payload}")
            
            # Make the webhook call
            async with httpx.AsyncClient(timeout=self.webhook_timeout) as client:
                response = await client.post(
                    self.webhook_url,
                    json=webhook_payload,
                    headers={"Content-Type": "application/json"}
                )
                response.raise_for_status()
                
                # Process the response
                return self._process_webhook_response(response)
                
        except httpx.TimeoutException:
            error_msg = f"Webhook call timed out after {self.webhook_timeout} seconds"
            logger.error(error_msg)
            raise Exception(error_msg)
            
        except httpx.HTTPStatusError as e:
            error_msg = f"Webhook HTTP error: {e.response.status_code} - {e.response.text}"
            logger.error(error_msg)
            raise Exception(error_msg)
            
        except Exception as e:
            error_msg = f"Unexpected error calling webhook: {str(e)}"
            logger.error(error_msg)
            raise Exception(error_msg)
    
    def _process_webhook_response(self, response: httpx.Response) -> str:
        """
        Process webhook response and extract the meaningful content.
        Handles both single JSON responses and N8N streamed responses.
        
        Args:
            response: The HTTP response from webhook
            
        Returns:
            Processed response text
        """
        response_text = response.text
        
        # Check if this looks like N8N streamed output (multiple lines with JSON objects)
        if self._is_streamed_response(response_text):
            return self._accumulate_streamed_content(response_text)
        
        try:
            # Try to parse as single JSON first
            webhook_data = response.json()
            logger.info(f"Webhook JSON response: {webhook_data}")
            
            if isinstance(webhook_data, dict):
                # Look for common response fields in order of preference
                response_fields = ["output", "message", "response", "text", "result", "content"]
                
                for field in response_fields:
                    if field in webhook_data:
                        response_text = str(webhook_data[field])
                        logger.info(f"Extracted response from field '{field}': {response_text[:100]}...")
                        return response_text
                
                # If no specific field found, return the whole object as formatted JSON
                logger.info("No standard response field found, returning full JSON")
                return json.dumps(webhook_data, ensure_ascii=False, indent=2)
            else:
                return str(webhook_data)
                
        except json.JSONDecodeError:
            # If not JSON, return as text
            logger.info(f"Webhook text response: {response_text}")
            return response_text
    
    def _is_streamed_response(self, response_text: str) -> bool:
        """
        Check if the response looks like N8N streamed output
        """
        lines = response_text.strip().split('\n')
        if len(lines) < 2:
            return False
            
        # Check if multiple lines contain JSON objects with "type" and "content" fields
        json_lines = 0
        for line in lines[:5]:  # Check first 5 lines
            try:
                data = json.loads(line.strip())
                if isinstance(data, dict) and "type" in data:
                    json_lines += 1
            except json.JSONDecodeError:
                continue
        
        return json_lines >= 2
    
    def _accumulate_streamed_content(self, response_text: str) -> str:
        """
        Accumulate content from N8N streamed response
        
        Expected format:
        {"type":"begin","metadata":...}
        {"type":"item","content":"Hello","metadata":...}
        {"type":"item","content":" world","metadata":...}
        {"type":"end","metadata":...}
        """
        accumulated_content = ""
        lines = response_text.strip().split('\n')
        
        logger.info(f"Processing {len(lines)} streamed response lines")
        
        for line in lines:
            line = line.strip()
            if not line:
                continue
                
            try:
                data = json.loads(line)
                
                # Only accumulate content from "item" type messages
                if (isinstance(data, dict) and 
                    data.get("type") == "item" and 
                    "content" in data):
                    
                    content = data["content"]
                    accumulated_content += content
                    
            except json.JSONDecodeError:
                logger.warning(f"Failed to parse line as JSON: {line[:100]}...")
                continue
        
        logger.info(f"Accumulated content: {accumulated_content}")
        return accumulated_content if accumulated_content.strip() else "No content received from webhook"


def create_agent():
    """
    Create and configure the webhook agent
    """
    import os
    
    webhook_url = os.getenv('WEBHOOK_URL')
    webhook_timeout = int(os.getenv('WEBHOOK_TIMEOUT', '120'))
    
    if not webhook_url:
        raise ValueError('WEBHOOK_URL environment variable is required')
    
    webhook_agent = WebhookAgent(webhook_url, webhook_timeout)
    
    return {
        'name': 'webhook_agent',
        'description': 'An agent that forwards messages to webhook endpoints',
        'version': '1.0.0',
        'webhook_agent': webhook_agent  # Pass the actual webhook agent instance
    }