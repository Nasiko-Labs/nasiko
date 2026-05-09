"""
Router orchestrator service that coordinates all router operations.
"""

import logging
import time
from collections.abc import AsyncGenerator
from typing import Any, Dict, List, Optional, Tuple

from router.src.core import (
    AgentRegistry,
    AgentRegistryError,
    VectorStoreService,
    VectorStoreError,
    AgentClient,
    AgentClientError,
    SessionHistoryService,
    AgentResponseCache,
    AgentRateLimiter,
    AgentHealthTracker,
    EventEmitter,
)
from router.src.config import settings
from router.src.entities import UserRequest, RouterResponse, RouterOutput
from router.src.core.routing_engine import router
from router.src.utils import truncate_agent_cards

logger = logging.getLogger(__name__)


class RouterOrchestrator:
    """Main orchestrator service for router operations."""

    def __init__(
        self,
        emitter: EventEmitter,
        cache: Optional[AgentResponseCache] = None,
        rate_limiter: Optional[AgentRateLimiter] = None,
        health: Optional[AgentHealthTracker] = None,
    ):
        if emitter is None:
            raise TypeError("RouterOrchestrator requires an EventEmitter — pass emitter=")
        self.agent_registry = AgentRegistry()
        self.session_history_service = SessionHistoryService()
        self.vector_store = VectorStoreService()
        self.agent_client = AgentClient()
        self.emitter = emitter
        self.cache = cache
        self.rate_limiter = rate_limiter
        self.health = health

    async def process_request(
        self,
        request: UserRequest,
        files: List[Tuple[str, Tuple[str, bytes, str]]],
        token: str,
    ) -> AsyncGenerator[str, None]:
        """
        Process a user request through the complete router pipeline.

        Yields:
            Router response messages as JSON strings
        """
        try:
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

        # Step 5: Route selection — retain full candidate list for health-based fallback
        try:
            _, _, candidates, router_output = router(
                request.query, conversation_history, truncated_agent_cards, vectorstore
            )
            # candidates = second_shortlist (reranked by history)
            # router_output = LLM-selected primary agent

            logger.info(f"Router selected agent: {router_output}")

            primary_agent = (
                router_output.agent_name
                if isinstance(router_output, RouterOutput)
                else router_output.get("name", "unknown")
            )

            await self.emitter.emit("agent_selected", agent=primary_agent)
            yield self._router_response(
                f"Agent selected to serve user's query: {router_output}", primary_agent
            )

        except Exception as e:
            error_msg = f"Agent routing failed: {str(e)}"
            logger.error(error_msg)
            yield self._router_response(error_msg, "", False, "")
            return

        # Step 6: Cache + rate-limit + health-aware dispatch
        try:
            async for response in self._send_agent_request(
                request, files, agent_cards, primary_agent, candidates or [], token
            ):
                yield response
        except Exception as e:
            error_msg = f"Failed to communicate with selected agent: {str(e)}"
            logger.error(error_msg)
            yield self._router_response(error_msg, "", False, "")

    async def _send_agent_request(
        self,
        request: UserRequest,
        files: List[Tuple[str, Tuple[str, bytes, str]]],
        agent_cards: List[Dict[str, Any]],
        primary_agent: str,
        candidates: List[str],
        token: str,
    ) -> AsyncGenerator[str, None]:
        """
        Cache check → stampede lock → health-based fallback → rate limit → agent call
        → health record → cache store.
        """
        has_files = bool(files)

        # ------------------------------------------------------------------
        # [1] Cache check with stampede protection
        # ------------------------------------------------------------------
        lock_acquired = False
        if self.cache:
            cached, lock_acquired = await self.cache.get_with_stampede_lock(
                primary_agent, request.query, has_files
            )
            if cached is not None:
                logger.info(f"Cache HIT for agent '{primary_agent}'")
                yield self._router_response(
                    self.agent_client.extract_response_content(cached),
                    primary_agent,
                    False,
                    "",
                )
                return

        # ------------------------------------------------------------------
        # [2] Health-based agent selection (fallback to candidate if primary degraded)
        # ------------------------------------------------------------------
        agent_name = primary_agent
        agent_url = self.agent_registry.get_agent_url(agent_cards, agent_name)

        if self.health and agent_url:
            try:
                queue_depth = 0
                if self.rate_limiter:
                    try:
                        queue_depth = int(
                            await self.rate_limiter.redis.zcard(
                                f"queue:agent:{agent_name}"
                            )
                        )
                    except Exception:
                        pass
                score = await self.health.get_score(agent_name, queue_depth)
                if score < settings.HEALTH_SCORE_THRESHOLD:
                    logger.warning(
                        f"Agent '{agent_name}' health score {score:.2f} below threshold "
                        f"{settings.HEALTH_SCORE_THRESHOLD} — trying fallback"
                    )
                    for alt in candidates:
                        if alt == primary_agent:
                            continue
                        alt_url = self.agent_registry.get_agent_url(agent_cards, alt)
                        if alt_url:
                            agent_name, agent_url = alt, alt_url
                            await self.emitter.emit(
                                "fallback_triggered",
                                agent=alt,
                                primary=primary_agent,
                                health_score=round(score, 3),
                            )
                            yield self._router_response(
                                f"Primary agent degraded (score {score:.2f}), routing to '{alt}'",
                                agent_name,
                            )
                            break
            except Exception as e:
                logger.warning(f"Health check error (continuing with primary): {e}")

        if not agent_url:
            if self.cache and lock_acquired:
                await self.cache.release_lock(primary_agent, request.query)
            yield self._router_response("No agents with valid URLs found", "", False, "")
            return

        # ------------------------------------------------------------------
        # [3] Rate limiting (agent-level + user-level sliding window)
        # ------------------------------------------------------------------
        if self.rate_limiter:
            allowed, retry_after = await self.rate_limiter.acquire(agent_name, token)
            if not allowed:
                if self.cache and lock_acquired:
                    await self.cache.release_lock(primary_agent, request.query)
                retry_msg = (
                    f"Agent '{agent_name}' is at capacity. "
                    f"Retry after {retry_after:.1f}s"
                    if retry_after
                    else f"Agent '{agent_name}' is at capacity. Please retry later."
                )
                logger.warning(retry_msg)
                yield self._router_response(retry_msg, agent_name, False, agent_url)
                return

        # ------------------------------------------------------------------
        # [4] Call agent + track health
        # ------------------------------------------------------------------
        yield self._router_response(
            "Sending user's query to agent...", "", False, agent_url
        )

        start = time.monotonic()
        try:
            logger.info(f"Sending request to agent: {agent_url}")
            agent_data = await self.agent_client.send_request(
                agent_url, request, files, token
            )
            latency_ms = (time.monotonic() - start) * 1000

            if self.health:
                await self.health.record(agent_name, True, latency_ms)

        except AgentClientError as e:
            latency_ms = (time.monotonic() - start) * 1000
            if self.health:
                await self.health.record(agent_name, False, latency_ms)
            if self.cache and lock_acquired:
                await self.cache.release_lock(primary_agent, request.query)
            yield self._router_response(str(e), agent_name, False, agent_url)
            return

        # ------------------------------------------------------------------
        # [5] Cache store + release stampede lock
        # ------------------------------------------------------------------
        if self.cache and lock_acquired and not has_files:
            await self.cache.set(primary_agent, request.query, agent_data)
            await self.cache.release_lock(primary_agent, request.query)

        agent_response = self.agent_client.extract_response_content(agent_data)
        logger.info("Successfully received response from agent")
        await self.emitter.emit(
            "request_completed",
            agent=agent_name,
            latency_ms=round(latency_ms, 1),
            cached=False,
        )
        yield self._router_response(agent_response, agent_name, False, agent_url)

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
        return {
            "router": "healthy",
            "timestamp": time.time(),
            "components": {
                "vector_store": "healthy",
                "agent_registry": "healthy",
                "agent_client": "healthy",
                "cache": "enabled" if self.cache else "disabled",
                "rate_limiter": "enabled" if self.rate_limiter else "disabled",
                "health_tracker": "enabled" if self.health else "disabled",
            },
        }
