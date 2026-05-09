from __future__ import annotations

import time

from router.src.resilience.models import AgentLimitState, LimitConfig, LimitDecision


class AdaptiveRateLimiter:
    """Per-agent adaptive token bucket.

    The limiter admits requests while tokens are available, queues short bursts,
    and rejects only when the queue is already saturated.
    """

    def __init__(self, default_config: LimitConfig | None = None):
        self.default_config = default_config or LimitConfig()
        self._configs: dict[str, LimitConfig] = {}
        self._states: dict[str, AgentLimitState] = {}
        self._current_limits: dict[str, float] = {}

    def check(self, agent_id: str, queue_depth: int) -> LimitDecision:
        config = self.get_config(agent_id)
        state = self._state_for(agent_id, config)
        effective_rps = self.effective_rps(agent_id, queue_depth)
        self._refill(state, config, effective_rps)

        if state.tokens >= 1:
            state.tokens -= 1
            return LimitDecision(
                action="allow",
                agent_id=agent_id,
                effective_rps=effective_rps,
                reason="token_available",
            )

        retry_after = max(1 / effective_rps, 0.001)
        if queue_depth >= config.max_queue_depth:
            return LimitDecision(
                action="reject",
                agent_id=agent_id,
                effective_rps=effective_rps,
                retry_after_seconds=min(config.max_queue_wait_seconds, retry_after),
                reason="queue_full",
            )

        return LimitDecision(
            action="queue",
            agent_id=agent_id,
            effective_rps=effective_rps,
            retry_after_seconds=retry_after,
            reason="token_depleted",
        )

    def record_result(
        self, agent_id: str, *, latency_seconds: float, success: bool
    ) -> None:
        config = self.get_config(agent_id)
        state = self._state_for(agent_id, config)
        state.recent_latencies.append(max(0.0, latency_seconds))
        state.recent_errors.append(0 if success else 1)
        del state.recent_latencies[:-20]
        del state.recent_errors[:-20]

    def effective_rps(self, agent_id: str, queue_depth: int) -> float:
        config = self.get_config(agent_id)
        state = self._state_for(agent_id, config)
        latency_pressure = self._latency_pressure(state, config)
        error_pressure = self._error_pressure(state)
        queue_pressure = self._queue_pressure(queue_depth, config)
        load_factor = min(
            0.95,
            latency_pressure * config.latency_weight
            + error_pressure * config.error_weight
            + queue_pressure * config.queue_weight,
        )
        effective = max(config.min_rps, config.base_rps * (1 - load_factor))
        self._current_limits[agent_id] = effective
        return effective

    def update_config(self, agent_id: str, **updates: object) -> LimitConfig:
        data = self.get_config(agent_id).model_dump()
        data.update({key: value for key, value in updates.items() if value is not None})
        config = LimitConfig(**data)
        self._configs[agent_id] = config
        state = self._state_for(agent_id, config)
        state.tokens = min(state.tokens, config.burst)
        return config

    def get_config(self, agent_id: str) -> LimitConfig:
        return self._configs.get(agent_id, self.default_config)

    def current_limits(self) -> dict[str, float]:
        return dict(self._current_limits)

    def _state_for(self, agent_id: str, config: LimitConfig) -> AgentLimitState:
        state = self._states.get(agent_id)
        if state is None:
            state = AgentLimitState(tokens=float(config.burst), last_refill=time.monotonic())
            self._states[agent_id] = state
        return state

    def _refill(
        self, state: AgentLimitState, config: LimitConfig, effective_rps: float
    ) -> None:
        now = time.monotonic()
        elapsed = max(0.0, now - state.last_refill)
        state.last_refill = now
        state.tokens = min(float(config.burst), state.tokens + elapsed * effective_rps)

    def _latency_pressure(
        self, state: AgentLimitState, config: LimitConfig
    ) -> float:
        if not state.recent_latencies:
            return 0.0
        average_latency = sum(state.recent_latencies) / len(state.recent_latencies)
        if average_latency <= config.target_latency_seconds:
            return 0.0
        return min(
            1.0,
            (average_latency - config.target_latency_seconds)
            / config.target_latency_seconds,
        )

    def _error_pressure(self, state: AgentLimitState) -> float:
        if not state.recent_errors:
            return 0.0
        return min(1.0, sum(state.recent_errors) / len(state.recent_errors))

    def _queue_pressure(self, queue_depth: int, config: LimitConfig) -> float:
        if config.max_queue_depth == 0:
            return 1.0 if queue_depth > 0 else 0.0
        return min(1.0, queue_depth / config.max_queue_depth)
