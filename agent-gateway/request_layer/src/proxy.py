"""Request pipeline orchestrator."""
import asyncio
import logging
from datetime import datetime, timezone

from fastapi import Request, Response

from request_layer.src import coalesce, forward
from request_layer.src.cache import exact as exact_cache
from request_layer.src.cache import router_cache
from request_layer.src.cache import semantic as semantic_cache
from request_layer.src.cache.policy import resolve as resolve_policy
from request_layer.src.config import Settings
from request_layer.src.phoenix import (
    cache_hit_span,
    coalesce_follower_span,
    queue_span,
)
from request_layer.src.ratelimit import (
    check_cost,
    check_rps,
    estimate_cost,
    record_cost,
)
from request_layer.src.types import CacheEntry, CacheEvent, RoutingDecision

logger = logging.getLogger(__name__)


class ProxyPipeline:
    """Holds the long-lived dependencies the pipeline needs.

    A single instance is created at FastAPI startup and shared across all
    inbound requests. It owns the Redis handle, the HTTP forwarder, the
    capability adapter, and the SSE event sink.
    """

    def __init__(
        self,
        *,
        settings: Settings,
        redis,
        forwarder: forward.Forwarder,
        adapter,
        event_sink,
    ) -> None:
        self.settings = settings
        self.redis = redis
        self.forwarder = forwarder
        self.adapter = adapter
        self.event_sink = event_sink
        # Aggregate counters surfaced on /admin/stats. Held in process for
        # cheap reads; Redis holds the persisted source of truth for cost.
        self.counters: dict[str, float] = {
            "requests_total": 0,
            "hits_l1": 0,
            "hits_l2": 0,
            "hits_l3": 0,
            "router_calls_skipped": 0,
            "coalesced_followers": 0,
            "queued": 0,
            "savings_usd": 0.0,
            "savings_ms": 0.0,
        }

    # ------------------------------------------------------------------ helpers

    async def _policy_for(self, agent: str):
        return await resolve_policy(self.redis, self.adapter.policies, agent)

    def _record_event(self, event: CacheEvent) -> None:
        self.event_sink.publish(event)

    # ------------------------------------------------------------------ agent path

    async def handle_agent_request(
        self,
        agent: str,
        path: str,
        request: Request,
    ) -> Response:
        """Drive an inbound ``/{agent}/{path}`` request through the pipeline."""

        self.counters["requests_total"] += 1

        body = await request.body()
        normalized = _normalize_payload(body)
        query_hash = exact_cache.stable_hash(normalized)
        method = request.method.upper()

        manifest = self.adapter.manifests.get(agent)
        policy = await self._policy_for(agent)

        # L1 — exact cache (only for idempotent methods + non-empty bodies)
        cacheable = method in {"GET", "POST"} and normalized != ""

        if cacheable:
            l1_entry = await exact_cache.get(self.redis, agent, normalized)
            if l1_entry is not None:
                self.counters["hits_l1"] += 1
                self._emit_hit_event(agent, "L1", l1_entry, similarity=1.0)
                return _entry_to_response(l1_entry, layer="L1")

        # L2 — semantic cache
        if cacheable:
            try:
                hit = await semantic_cache.lookup(
                    self.redis, agent, normalized, policy.semantic_threshold
                )
            except Exception:  # noqa: BLE001
                logger.exception("semantic lookup failed for %s; treating as miss", agent)
                hit = None
            if hit is not None:
                entry, similarity, matched = hit
                self.counters["hits_l2"] += 1
                self._emit_hit_event(
                    agent, "L2", entry, similarity=similarity, matched=matched
                )
                return _entry_to_response(entry, layer="L2", similarity=similarity)

        # L4 — coalescer
        if cacheable:
            async with coalesce.acquire_leader(
                self.redis,
                agent,
                query_hash,
                self.settings.request_layer_coalesce_wait_seconds,
            ) as is_leader:
                if not is_leader:
                    self.counters["coalesced_followers"] += 1
                    with coalesce_follower_span(agent, query_hash):
                        payload = await coalesce.wait_for_broadcast(
                            self.redis,
                            agent,
                            query_hash,
                            self.settings.request_layer_coalesce_wait_seconds,
                        )
                    if payload is not None:
                        try:
                            entry = CacheEntry.model_validate_json(payload)
                            return _entry_to_response(entry, layer="coalesce")
                        except ValueError:
                            pass
                    # Fall through and treat as a fresh request.
                    return await self._forward_with_gates(
                        agent, path, request, body, normalized, manifest, policy
                    )

                # Leader path
                response, entry = await self._forward_with_gates(
                    agent,
                    path,
                    request,
                    body,
                    normalized,
                    manifest,
                    policy,
                    return_entry=True,
                )
                if entry is not None and 200 <= entry.status_code < 300:
                    await coalesce.broadcast(
                        self.redis, agent, query_hash, entry.model_dump_json()
                    )
                return response

        return await self._forward_with_gates(
            agent, path, request, body, normalized, manifest, policy
        )

    async def _forward_with_gates(
        self,
        agent: str,
        path: str,
        request: Request,
        body: bytes,
        normalized: str,
        manifest,
        policy,
        return_entry: bool = False,
    ):
        # L5a — RPS bucket
        verdict = await check_rps(self.redis, agent, policy.rps_limit)
        if not verdict.allowed:
            with queue_span("request_layer.queue.entry", agent, "normal"):
                self.counters["queued"] += 1
                await asyncio.sleep(min(2.0, verdict.retry_after_ms / 1000.0))

        # L5b — cost cap
        if not await check_cost(self.redis, agent, policy.cost_cap_usd_per_min):
            self.counters["queued"] += 1
            await asyncio.sleep(0.5)

        # L6 — forward
        target = _build_agent_url(self.settings, manifest, agent, path)
        result = await self.forwarder.forward(
            method=request.method,
            url=target,
            headers=dict(request.headers),
            body=body,
            params=list(request.query_params.multi_items()),
        )

        # Compute cost / savings
        model = manifest.model if manifest else None
        cost_usd = estimate_cost(
            model=model,
            body_in=body,
            body_out=result.body,
            headers=result.headers,
        )
        await record_cost(self.redis, agent, cost_usd)

        entry = exact_cache.serialize_entry(
            status_code=result.status_code,
            headers=_safe_headers(result.headers),
            body=result.body,
            cost_usd=cost_usd,
            latency_ms=result.latency_ms,
        )

        # L7 — cache fill (success only; never poison-cache a 5xx)
        cacheable = (
            request.method.upper() in {"GET", "POST"}
            and normalized != ""
            and 200 <= result.status_code < 300
        )
        if cacheable:
            await self._fill_caches(agent, normalized, entry, policy)

        response = Response(
            content=result.body,
            status_code=result.status_code,
            headers=_safe_headers(result.headers),
        )
        response.headers["X-Request layer-Layer"] = "origin"
        if return_entry:
            return response, entry
        return response

    async def _fill_caches(
        self,
        agent: str,
        normalized: str,
        entry: CacheEntry,
        policy,
    ) -> None:
        await exact_cache.set(
            self.redis, agent, normalized, entry, policy.cache_ttl_seconds
        )
        try:
            await semantic_cache.ensure_index(
                self.redis, agent, self.settings.request_layer_embedding_dim
            )
            await semantic_cache.store(
                self.redis, agent, normalized, entry, policy.cache_ttl_seconds
            )
        except Exception:  # noqa: BLE001 — semantic cache is best-effort
            logger.exception("failed to write L2 entry for %s", agent)

    # ------------------------------------------------------------------ router path

    async def handle_router_request(self, request: Request) -> Response | None:
        """Optional L3 short-circuit for ``/router/route``.

        Returns ``None`` when L3 misses or is disabled — the caller should
        fall through to the regular Nasiko router service in that case.
        """

        if not self.settings.request_layer_router_cache_enabled:
            return None

        query = request.query_params.get("query")
        if not query:
            return None
        normalized = query.strip().lower()

        decision = await router_cache.lookup(
            self.redis, normalized, self.settings.request_layer_router_cache_threshold
        )
        if decision is None:
            return None

        self.counters["hits_l3"] += 1
        self.counters["router_calls_skipped"] += 1
        # Estimate router-call savings: one LLM call (~500ms, ~$0.0003).
        self.counters["savings_usd"] += 0.0003
        self.counters["savings_ms"] += 500
        with cache_hit_span(
            layer="L3",
            agent=decision.agent_name,
            similarity=decision.confidence,
            matched_query=decision.matched_query,
            savings_usd=0.0003,
            savings_ms=500,
            router_skipped=True,
        ):
            pass

        body = decision.model_dump_json().encode("utf-8")
        response = Response(
            content=body,
            status_code=200,
            headers={"Content-Type": "application/json"},
        )
        response.headers["X-Request layer-Layer"] = "L3"
        response.headers["X-Request layer-Router-Skipped"] = "true"
        self._record_event(
            CacheEvent(
                timestamp=datetime.now(timezone.utc),
                agent=decision.agent_name,
                layer="L3",
                similarity=decision.confidence,
                matched_query=decision.matched_query,
                savings_usd=0.0003,
                savings_ms=500.0,
                router_skipped=True,
            )
        )
        return response

    async def remember_routing_decision(
        self,
        normalized_query: str,
        decision: RoutingDecision,
    ) -> None:
        """Cache a routing decision after the router service has computed it."""

        if not self.settings.request_layer_router_cache_enabled:
            return
        try:
            await router_cache.ensure_index(
                self.redis, self.settings.request_layer_embedding_dim
            )
            await router_cache.store(
                self.redis,
                normalized_query,
                decision,
                ttl_seconds=self.settings.request_layer_default_ttl_seconds,
            )
        except Exception:  # noqa: BLE001
            logger.exception("failed to store L3 entry")

    # ------------------------------------------------------------------ emission

    def _emit_hit_event(
        self,
        agent: str,
        layer: str,
        entry: CacheEntry,
        *,
        similarity: float,
        matched: str | None = None,
    ) -> None:
        now = datetime.now(timezone.utc)
        cached_at = entry.cached_at if entry.cached_at.tzinfo else entry.cached_at.replace(tzinfo=timezone.utc)
        age_seconds = max(0.0, (now - cached_at).total_seconds())
        savings_ms = entry.latency_ms
        savings_usd = entry.cost_usd
        self.counters["savings_usd"] += savings_usd
        self.counters["savings_ms"] += savings_ms

        with cache_hit_span(
            layer=layer,
            agent=agent,
            similarity=similarity,
            matched_query=matched or entry.matched_query,
            age_seconds=age_seconds,
            savings_usd=savings_usd,
            savings_ms=savings_ms,
        ):
            pass

        self._record_event(
            CacheEvent(
                timestamp=now,
                agent=agent,
                layer=layer,
                similarity=similarity,
                matched_query=matched or entry.matched_query,
                savings_usd=savings_usd,
                savings_ms=savings_ms,
            )
        )


# ============================================================================ helpers


def _normalize_payload(body: bytes) -> str:
    from request_layer.src.normalize import normalize as _normalize

    return _normalize(body)


def _entry_to_response(
    entry: CacheEntry,
    *,
    layer: str,
    similarity: float | None = None,
) -> Response:
    body = exact_cache.decode_body(entry)
    response = Response(
        content=body,
        status_code=entry.status_code,
        headers=entry.headers,
    )
    response.headers["X-Request layer-Layer"] = layer
    if similarity is not None:
        response.headers["X-Request layer-Similarity"] = f"{similarity:.4f}"
    if entry.matched_query is not None:
        response.headers["X-Request layer-Matched"] = entry.matched_query[:120]
    response.headers["X-Request layer-Cached-At"] = entry.cached_at.isoformat()
    return response


def _safe_headers(headers: dict[str, str]) -> dict[str, str]:
    """Strip headers that don't survive cache round-trips."""

    return {
        k: v
        for k, v in headers.items()
        if k.lower()
        not in {"transfer-encoding", "content-length", "connection", "content-encoding"}
    }


def _build_agent_url(
    settings: Settings,
    manifest,
    agent: str,
    path: str,
) -> str:
    """Compute the target URL for the agent.

    Request layer forwards through Kong (so the existing service registry, auth
    plugins, and observability hooks still apply). Kong proxies
    ``/agents/{name}/{path}``; we synthesize that URL here.
    """

    base = settings.request_layer_kong_proxy_internal.rstrip("/")
    cleaned = path.lstrip("/")
    return f"{base}/agents/{agent}/{cleaned}"


