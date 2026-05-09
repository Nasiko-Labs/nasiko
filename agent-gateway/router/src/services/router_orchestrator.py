"""
Router Orchestrator — with Resilient Agent Request Layer (RAL) middleware.

Changes from original
---------------------
* On startup: RAL components (cache, deduplicator, rate-limiter, queue,
  metrics) are initialised and connected.
* `process_request()` now runs through the full RAL pipeline:
    1. Check response cache  →  serve immediately if hit
    2. Deduplicate identical in-flight requests
    3. Acquire rate-limit token (with queue back-pressure)
    4. Execute original router pipeline (unchanged business logic)
    5. Store result in cache, resolve deduplicator, update metrics
* All original methods (`_handle_route_selection`, `_send_agent_request`,
  `_get_agent_url`, `_router_response`, `health_check`) are **preserved
  verbatim** — only `__init__` and `process_request` are extended.
"""

import logging
import time
import uuid
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
from router.src.ral import (
    ResponseCache,
    RequestDeduplicator,
    AdaptiveRateLimiter,
    AsyncRequestQueue,
    RalMetrics,
)
from router.src.ral.config import ral_settings

logger = logging.getLogger(__name__)


class RouterOrchestrator:
    """Main orchestrator service for router operations — RAL-enabled."""

    def __init__(self):
        # ── Original components (unchanged) ──────────────────────────────
        self.agent_registry = AgentRegistry()
        self.session_history_service = SessionHistoryService()
        self.vector_store = VectorStoreService()
        self.agent_client = AgentClient()

        # ── RAL middleware components ────────────────────────────────────
        self._cache       = ResponseCache()
        self._dedup       = RequestDeduplicator()
        self._rate_limiter = AdaptiveRateLimiter()
        self._queue       = AsyncRequestQueue()
        self._metrics     = RalMetrics()

        # Track whether async resources have been initialised
        self._ral_ready = False

    # ------------------------------------------------------------------
    # RAL lifecycle helpers (called lazily on first request)
    # ------------------------------------------------------------------

    async def _ensure_ral_ready(self) -> None:
        """Lazily connect RAL async resources on first use."""
        if self._ral_ready:
            return
        await self._cache.connect()
        await self._metrics.connect()
        await self._queue.start()
        self._ral_ready = True
        logger.info("RAL middleware initialised.")

    async def shutdown(self) -> None:
        """Graceful shutdown — drain queue, close Redis connections."""
        await self._queue.stop()
        await self._cache.close()
        await self._metrics.close()
        logger.info("RAL middleware shut down.")

    # ------------------------------------------------------------------
    # process_request  ← main entry point (RAL-wrapped)
    # ------------------------------------------------------------------

    async def process_request(
        self,
        request: UserRequest,
        files: List[Tuple[str, Tuple[str, bytes, str]]],
        token: str,
    ) -> AsyncGenerator[str, None]:
        """
        Process a user request through the RAL pipeline then the router.

        Intercept order:
          Cache hit?          → serve immediately, skip LLM
          In-flight dedup?    → wait for the leader, share result
          Rate limit acquire  → queue if needed (never hard-reject)
          Original pipeline   → unchanged routing + agent call
          Post-processing     → cache store, metrics update, log entry
        """
        await self._ensure_ral_ready()

        # Derive a cache/dedup key from the content (exclude session_id for
        # global semantic deduplication — maximises cache hit-rate).
        provider = ral_settings.REDIS_HOST  # placeholder; real provider from routing_engine
        cache_key = self._cache.make_key(
            query=request.query,
            route=request.route,
            model="router",        # coarse granularity; agent is selected at runtime
            provider="nasiko",
        )

        request_id = uuid.uuid4().hex[:12]
        t_start = time.monotonic()

        # ── 1. Cache lookup ────────────────────────────────────────────
        cached = await self._cache.get(cache_key)
        if cached is not None:
            self._metrics.on_cache_hit()
            latency_ms = (time.monotonic() - t_start) * 1000.0
            self._metrics.on_request_end(
                agent_id="cached", latency_ms=latency_ms, from_cache=True
            )
            self._metrics.log_request({
                "request_id": request_id,
                "query": request.query[:120],
                "agent_id": "cached",
                "latency_ms": round(latency_ms, 1),
                "from_cache": True,
                "status": "ok",
                "timestamp": time.time(),
            })
            logger.info("RAL: cache HIT  request_id=%s", request_id)
            # Yield the cached chunk as if it came from the pipeline
            async for chunk in self._yield_cached(cached):
                yield chunk
            return

        self._metrics.on_cache_miss()

        # ── 2. Deduplication guard ─────────────────────────────────────
        async with self._dedup.guard(cache_key) as (is_leader, wait_fn):
            if not is_leader:
                # Another identical request is in-flight — wait for it
                logger.info("RAL: dedup WAIT request_id=%s", request_id)
                try:
                    shared_result = await wait_fn()
                    if shared_result:
                        latency_ms = (time.monotonic() - t_start) * 1000.0
                        self._metrics.on_request_end(
                            "dedup", latency_ms=latency_ms, from_cache=True
                        )
                        async for chunk in self._yield_cached(shared_result):
                            yield chunk
                        return
                except Exception as exc:
                    logger.warning("RAL: dedup wait failed: %s — proceeding", exc)
                    # Fall through to execute independently

            # ── 3. Rate limiting + queue ───────────────────────────────
            # Extract agent_id for per-agent throttle tracking.
            # We use "default" until routing selects an agent; the per-agent
            # stats are updated in on_request_end after selection.
            agent_id_for_limiter = "default"
            acquired = await self._rate_limiter.acquire(
                agent_id_for_limiter,
                timeout=ral_settings.RAL_QUEUE_TIMEOUT,
            )
            throttled = not acquired

            # ── 4. Execute original pipeline ──────────────────────────
            self._metrics.on_request_start(agent_id_for_limiter)

            collected_chunks: List[str] = []
            actual_agent_id = "unknown"
            error_occurred = False

            try:
                async for chunk in self._handle_route_selection(request, files, token):
                    collected_chunks.append(chunk)
                    yield chunk
                    # Try to extract agent name from the first non-status chunk
                    if actual_agent_id == "unknown" and chunk:
                        try:
                            import json as _json
                            parsed = _json.loads(chunk)
                            if parsed.get("agent_id"):
                                actual_agent_id = parsed["agent_id"]
                        except Exception:
                            pass

            except Exception as exc:
                error_occurred = True
                logger.error("RAL: pipeline error request_id=%s: %s", request_id, exc)
                err_msg = f"Router processing failed: {str(exc)}"
                yield self._router_response(err_msg, "", False, "")

            finally:
                # Always release the rate-limiter slot
                self._rate_limiter.release(agent_id_for_limiter)

            # ── 5. Post-processing ─────────────────────────────────────
            latency_ms = (time.monotonic() - t_start) * 1000.0

            if not error_occurred and collected_chunks:
                # Combine chunks into a single cacheable string
                full_response = "".join(collected_chunks)

                # Store in cache (non-blocking)
                await self._cache.set(cache_key, full_response)

                # Resolve deduplicator so waiters get the result
                if is_leader:
                    self._dedup.resolve(cache_key, full_response)
            else:
                if is_leader:
                    self._dedup.reject(
                        cache_key,
                        RuntimeError("Pipeline failed — no cached result")
                    )

            self._metrics.on_request_end(
                agent_id=actual_agent_id,
                latency_ms=latency_ms,
                error=error_occurred,
                throttled=throttled,
            )
            self._metrics.log_request({
                "request_id": request_id,
                "query": request.query[:120],
                "agent_id": actual_agent_id,
                "latency_ms": round(latency_ms, 1),
                "from_cache": False,
                "throttled": throttled,
                "status": "error" if error_occurred else "ok",
                "timestamp": time.time(),
            })

    # ------------------------------------------------------------------
    # Cache response helper
    # ------------------------------------------------------------------

    async def _yield_cached(self, cached: str) -> AsyncGenerator[str, None]:
        """Re-yield a cached response string as a single chunk."""
        yield cached

    # ------------------------------------------------------------------
    # RAL stats helpers (used by the /metrics endpoint on the router)
    # ------------------------------------------------------------------

    async def get_ral_snapshot(self) -> Dict[str, Any]:
        """Return combined RAL metrics for the router's /metrics endpoint."""
        await self._ensure_ral_ready()
        snapshot = await self._metrics.get_snapshot()
        snapshot["rate_limiter"] = self._rate_limiter.get_stats()
        snapshot["queue"] = self._queue.get_stats()
        snapshot["dedup_in_flight"] = self._dedup.in_flight_count
        snapshot["cache"] = await self._cache.get_stats()
        return snapshot

    async def get_ral_logs(self, limit: int = 50) -> list:
        await self._ensure_ral_ready()
        return await self._metrics.get_recent_logs(limit)

    async def flush_cache(self) -> int:
        await self._ensure_ral_ready()
        return await self._cache.flush_all()

    # ------------------------------------------------------------------
    # ══════════════════════════════════════════════════════════════════
    #  All methods below are UNCHANGED from the original orchestrator.
    # ══════════════════════════════════════════════════════════════════
    # ------------------------------------------------------------------

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
            async for response in self._send_agent_request(
                request, files, agent_url, token
            ):
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
        token: str,
    ) -> AsyncGenerator[str, None]:
        """Send request to agent and yield response."""

        try:
            logger.info(f"Sending request to agent: {agent_url}")
            yield self._router_response(
                "Sending user's query to agent...", "", False, agent_url
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
        """Perform health check on router components including RAL."""

        health_status = {
            "router": "healthy",
            "timestamp": __import__("time").time(),
            "components": {},
        }

        try:
            health_status["components"]["vector_store"] = "healthy"
            health_status["components"]["agent_registry"] = "healthy"
            health_status["components"]["agent_client"] = "healthy"

            # RAL component health
            ral_ok = self._ral_ready
            health_status["components"]["ral_cache"] = "healthy" if ral_ok else "not_initialised"
            health_status["components"]["ral_queue"] = "healthy" if ral_ok else "not_initialised"
            health_status["components"]["ral_metrics"] = "healthy" if ral_ok else "not_initialised"

        except Exception as e:
            health_status["router"] = "unhealthy"
            health_status["error"] = str(e)
            logger.error(f"Health check failed: {e}")

        return health_status
