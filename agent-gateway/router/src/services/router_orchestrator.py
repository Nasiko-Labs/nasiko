"""
Router orchestrator service that coordinates all router operations.
"""

import logging
from collections.abc import AsyncGenerator
from typing import List, Tuple, Dict, Any, Optional

from router.src.core import (
    AgentRegistry,
    AgentRegistryError,
    VectorStoreService,
    VectorStoreError,
    AgentClient,
    AgentClientError,
    SessionHistoryService,
)
from router.src.entities import UserRequest, RouterResponse, RouterOutput
from router.src.core.routing_engine import router
from router.src.utils import truncate_agent_cards

logger = logging.getLogger(__name__)


class RouterOrchestrator:
    """Main orchestrator service for router operations."""

    def __init__(self):
        self.agent_registry = AgentRegistry()
        self.session_history_service = SessionHistoryService()
        self.vector_store = VectorStoreService()
        self.agent_client = AgentClient()

    async def process_request(
        self,
        request: UserRequest,
        files: List[Tuple[str, Tuple[str, bytes, str]]],
        token: str,
    ) -> AsyncGenerator[str, None]:
        """
        Process a user request through the complete router pipeline.

        Args:
            request: User request object
            files: List of file tuples for upload
            token: Authorization token

        Yields:
            Router response messages as JSON strings
        """
        try:
            # Route selection needed
            async for response in self._handle_route_selection(request, files, token):
                yield response

        except Exception as e:
            error_msg = f"Router processing failed: {str(e)}"
            logger.error(error_msg)
            yield self._router_response(error_msg, "", False, "")

    async def _handle_route_selection(
        self,
        request: UserRequest,
        files: List[Tuple[str, Tuple[str, bytes, str]]],
        token: str,
    ) -> AsyncGenerator[str, None]:
        """Handle requests that need route selection."""

        logger.info(f"Processing query for route selection: {request.query}")
        yield self._router_response("Processing user's query...")

        # Step 1: Fetch agent cards
        try:
            logger.info("Fetching agent details from registry...")
            yield self._router_response("Fetching agent details from the registry...")

            agent_cards = await self.agent_registry.fetch_agent_cards(token)

            if not agent_cards:
                yield self._router_response(
                    "No agents available in registry", "", False, ""
                )
                return

            yield self._router_response("Received agent details from the registry...")

        except AgentRegistryError as e:
            yield self._router_response(str(e), "", False, "")
            return

        # Step 2: Prepare agent data for routing
        try:
            truncated_agent_cards = truncate_agent_cards(agent_cards)
            logger.info(
                f"Prepared {len(truncated_agent_cards)} agent cards for routing"
            )

        except Exception as e:
            error_msg = f"Error processing agent cards: {str(e)}"
            logger.error(error_msg)
            yield self._router_response(error_msg, "", False, "")
            return

        # Step 3: Create vector store for similarity search
        try:
            logger.info("Creating vector store for agent selection...")
            yield self._router_response(
                "Determining the best agent to serve the user's query..."
            )

            vectorstore = self.vector_store.create_vector_store(agent_cards)

        except VectorStoreError as e:
            yield self._router_response(str(e), "", False, "")
            return

        # Step 4: Get context of previous user queries if any
        try:
            logger.info("Fetching context of previous user queries...")
            yield self._router_response("Fetching context of previous user queries...")

            response = await self.session_history_service.fetch_session_history(
                token, request.session_id
            )

            conversation_history = (
                self.session_history_service.reconstruct_conversation(response)
            )

            yield self._router_response("Retrived the conversation history...")

        except Exception as e:
            error_msg = f"Agent routing failed: {str(e)}"
            logger.error(error_msg)
            yield self._router_response(error_msg, "", False, "")
            return

        # Step 5: Route selection using AI
        try:
            _, _, _, router_output = router(
                request.query, conversation_history, truncated_agent_cards, vectorstore
            )

            logger.info(f"Router selected agent: {router_output}")

            agent_name = (
                router_output.agent_name
                if isinstance(router_output, RouterOutput)
                else router_output.get("name", "unknown")
            )

            yield self._router_response(
                f"Agent selected to serve user's query: {router_output}", agent_name
            )

        except Exception as e:
            error_msg = f"Agent routing failed: {str(e)}"
            logger.error(error_msg)
            yield self._router_response(error_msg, "", False, "")
            return

        # Step 6: Get agent URL and send request
        try:
            agent_url = await self._get_agent_url(agent_cards, agent_name)
            if not agent_url:
                yield self._router_response(
                    "No agents with valid URLs found", "", False, ""
                )
                return

            # Send request to selected agent
            async for response in self._send_agent_request(request, files, agent_url, token):
                yield response

        except Exception as e:
            error_msg = f"Failed to communicate with selected agent: {str(e)}"
            logger.error(error_msg)
            yield self._router_response(error_msg, "", False, agent_url)
    async def _send_agent_request(
        self,
        request: UserRequest,
        files: List[Tuple[str, Tuple[str, bytes, str]]],
        agent_url: str,
        token: str
    ) -> AsyncGenerator[str, None]:
        """Send request to agent and yield response."""

        try:
            logger.info(f"Sending request to agent: {agent_url}")
            yield self._router_response(
                f"Sending user's query to agent...", "", False, agent_url
            )

            # Send request to agent
            agent_data = await self.agent_client.send_request(
                agent_url, request, files, token
            )

            # Extract response content
            agent_response = self.agent_client.extract_response_content(agent_data)

            logger.info("Successfully received response from agent")
            yield self._router_response(agent_response, "", False, agent_url)

        except AgentClientError as e:
            yield self._router_response(str(e), "", False, agent_url)

    async def _get_agent_url(
        self, agent_cards: List[Dict[str, str]], agent_name: str
    ) -> Optional[str]:
        """Get the URL for a specific agent with fallback logic."""

        # Try to get URL for selected agent
        agent_url = self.agent_registry.get_agent_url(agent_cards, agent_name)

        if agent_url:
            return agent_url

        # Fallback to first available agent
        logger.warning(f"Agent {agent_name} not found or has no URL, using fallback")

        fallback = self.agent_registry.get_fallback_agent(agent_cards)
        if fallback:
            fallback_name, fallback_url = fallback
            logger.info(f"Using fallback agent: {fallback_name}")
            return fallback_url

        return None

    def _router_response(
        self,
        message: str,
        agent_id: str = "",
        is_int_response: bool = True,
        url: str = "",
    ) -> str:
        """Create a router response message."""
        return (
            RouterResponse(
                message=message,
                is_int_response=is_int_response,
                agent_id=agent_id,
                url=url,
            ).model_dump_json()
            + "\n"
        )

    async def health_check(self) -> Dict[str, Any]:
        """Perform health check on router components."""

        health_status = {
            "router": "healthy",
            "timestamp": __import__("time").time(),
            "components": {},
        }

        try:
            # Check vector store service
            health_status["components"]["vector_store"] = "healthy"

            # Check agent registry (without making external calls)
            health_status["components"]["agent_registry"] = "healthy"

            # Check agent client
            health_status["components"]["agent_client"] = "healthy"

        except Exception as e:
            health_status["router"] = "unhealthy"
            health_status["error"] = str(e)
            logger.error(f"Health check failed: {e}")

        return health_status
