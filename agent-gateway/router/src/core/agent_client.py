"""
Agent client service for communicating with selected agents.
"""

import logging
from typing import Dict, List, Tuple, Any, Optional

import httpx
from router.src.config import settings
from router.src.entities import UserRequest

logger = logging.getLogger(__name__)


class AgentClientError(Exception):
    """Custom exception for agent client errors."""

    pass


class AgentClient:
    """Service for communicating with agents."""

    def __init__(self):
        self.timeout = httpx.Timeout(settings.REQUEST_TIMEOUT)

    def _translate_agent_url(self, agent_url: str) -> str:
        """
        Translate external agent URLs to internal Docker network URLs for local deployment.
        
        Converts localhost:9100 (external Kong Gateway access) to kong-gateway:8000 
        (internal Docker network access) when running in local Docker deployment.
        
        Args:
            agent_url: Original agent URL from registry
            
        Returns:
            Translated URL suitable for internal container communication
        """
        if "localhost:9100" in agent_url:
            # Local Docker deployment: translate to internal Kong Gateway address
            return agent_url.replace("localhost:9100", "kong-gateway:8000")
        return agent_url

    async def send_request(
        self,
        agent_url: str,
        request: UserRequest,
        files: List[Tuple[str, Tuple[str, bytes, str]]],
        token: str,
    ) -> Dict[str, Any]:
        """
        Send a request to an agent.

        Args:
            agent_url: URL of the target agent
            request: User request object
            files: List of file tuples for upload
            token: Optional authorization token to forward to agent

        Returns:
            Agent response data

        Raises:
            AgentClientError: If the request fails
        """
        try:
            # Translate agent URL for internal Docker network communication
            translated_url = self._translate_agent_url(agent_url)
            payload = self._construct_payload(request, files, translated_url)

            logger.info(f"Sending request to agent: {agent_url} -> {translated_url}")
            logger.debug(f"Payload: {payload}")

            # Prepare headers for agent request
            headers = {}
            if token:
                headers["Authorization"] = f"Bearer {token}"
                logger.debug("Added Authorization header for agent request")

            async with httpx.AsyncClient(timeout=self.timeout) as client:
                response = await client.post(translated_url, json=payload, headers=headers)
                response.raise_for_status()

            data = response.json()

            if "error" in data:
                raise AgentClientError(f"Agent error: {data['error']}")

            if "result" not in data:
                raise AgentClientError("Invalid response: missing 'result' field")

            logger.info("Successfully received response from agent")
            return data

        except httpx.HTTPStatusError as e:
            error_msg = f"HTTP error from agent {translated_url}: {e.response.status_code} - {e.response.text}"
            logger.error(error_msg)
            raise AgentClientError(error_msg) from e

        except httpx.RequestError as e:
            error_msg = f"Request error to agent {translated_url}: {str(e)}"
            logger.error(error_msg)
            raise AgentClientError(error_msg) from e

        except Exception as e:
            error_msg = (
                f"Unexpected error communicating with agent {translated_url}: {str(e)}"
            )
            logger.error(error_msg)
            raise AgentClientError(error_msg) from e

    def _construct_payload(
        self,
        request: UserRequest,
        files: List[Tuple[str, Tuple[str, bytes, str]]],
        agent_url: str,
    ) -> Dict[str, Any]:
        """
        Construct the payload for agent request.

        Args:
            request: User request object
            files: List of file tuples
            agent_url: Target agent URL

        Returns:
            Payload dictionary
        """
        # Import here to avoid circular imports
        from router.src.utils import construct_payload

        return construct_payload(request, files, agent_url)

    def extract_response_content(self, agent_data: Dict[str, Any]) -> str:
        """
        Extract the text content from agent response.

        Args:
            agent_data: Response data from agent

        Returns:
            Extracted text content

        Raises:
            AgentClientError: If extraction fails
        """
        try:
            result = agent_data.get("result")

            if not result:
                raise AgentClientError("Invalid response: missing 'result' field")

            kind = result.get("kind")

            if kind == "message":
                return self._extract_text_from_message(result)
            elif kind == "task":
                artifacts = result.get("artifacts", [])
                if not artifacts:
                    raise AgentClientError("Task returned with empty history")
                last_msg = artifacts[-1]
                return self._extract_text_from_message(last_msg)
            else:
                raise AgentClientError(f"Unknown response kind: {kind}")

        except Exception as e:
            error_msg = f"Failed to extract response content: {str(e)}"
            logger.error(error_msg)
            raise AgentClientError(error_msg) from e

    def _extract_text_from_message(self, message: Dict[str, Any]) -> str:
        """
        Extract text from a message object.

        Args:
            message: Message dictionary

        Returns:
            Extracted text content
        """
        # Import here to avoid circular imports
        from router.src.utils import extract_text_from_message

        return extract_text_from_message(message)

    async def health_check(self, agent_url: str) -> bool:
        """
        Check if an agent is healthy and responding.

        Args:
            agent_url: URL of the agent to check

        Returns:
            True if agent is healthy, False otherwise
        """
        try:
            # Translate agent URL for internal Docker network communication
            translated_url = self._translate_agent_url(agent_url)
            
            # Construct health check URL (assuming /health endpoint)
            health_url = f"{translated_url.rstrip('/')}/health"

            async with httpx.AsyncClient(timeout=httpx.Timeout(10.0)) as client:
                response = await client.get(health_url)
                return response.status_code == 200

        except Exception as e:
            logger.warning(f"Health check failed for agent {agent_url}: {e}")
            return False
