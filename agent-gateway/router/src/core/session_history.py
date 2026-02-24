"""
Session history service for fetching session history for a user.
"""

import logging
from typing import Any, List, Dict

import httpx
from router.src.config import settings

logger = logging.getLogger(__name__)


class SessionHistoryError(Exception):
    """Custom exception for agent registry errors."""

    pass


class SessionHistoryService:
    """Service for interacting with the chat history service."""

    def __init__(self):
        self.timeout = httpx.Timeout(settings.REQUEST_TIMEOUT)

    async def fetch_session_history(
        self, token: str, session_id: str
    ) -> List[Dict[str, str]]:
        """
        Fetch session history.

        Args:
            token: Authorization token
            session_id: Session ID

        Returns:
            The session history

        Raises:
            SessionHistoryError: If fetching fails
        """

        chat_history_url = f"{settings.NASIKO_BACKEND}/chat/session/{session_id}"
        headers = {
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
        }

        logger.info(f"Fetching session history from {chat_history_url}")

        try:
            async with httpx.AsyncClient(timeout=self.timeout) as client:
                response = await client.get(chat_history_url, headers=headers)
                response.raise_for_status()
                data = response.json()

                self._validate_response(data)
                history = data["data"]

                logger.info(
                    f"Successfully fetched {len(history)} messages from chat history"
                )
                return history

        except httpx.HTTPStatusError as e:
            error_msg = f"HTTP error session history: {e.response.status_code} {e.response.text}"
            logger.error(error_msg)
            raise SessionHistoryError(error_msg) from e

        except httpx.RequestError as e:
            error_msg = f"Request error fetching session history: {e}"
            logger.error(error_msg)
            raise SessionHistoryError(error_msg) from e

        except Exception as e:
            error_msg = f"Unexpected error fetching session history: {e}"
            logger.error(error_msg)
            raise SessionHistoryError(error_msg) from e

    def _validate_response(self, data: Dict) -> None:
        """
        Validate the response from the chat history API.

        Args:
            data: The response data from the chat history API.

        Raises:
            ValueError: If the response is invalid.
        """
        if "data" not in data:
            raise ValueError("Invalid chat history: missing 'data' field")

        if not isinstance(data["data"], list):
            raise ValueError("Invalid chat history: 'data' field is not a list")

    def reconstruct_conversation(
        self, response: List[Dict[str, Any]]
    ) -> List[Dict[str, str]]:
        """
        Reconstruct a conversation from a chat history response.

        Args:
            response: The chat history response from the API.

        Returns:
            A list of dictionaries, where each dictionary contains the role and content of a message in the conversation.

        Raises:
            Exception: If there is an error reconstructing the conversation.
        """
        conversation = []
        try:
            for message in response:
                conversation.append(
                    {"role": message["role"], "content": message["content"]}
                )
            return conversation
        except Exception as e:
            logger.error(f"Error reconstructing conversation: {e}")
            return []
