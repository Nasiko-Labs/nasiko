"""Request layer: resilient agent request layer.

Sits between Kong and the agent fleet. Caches responses, coalesces concurrent
duplicate requests, applies per-agent rate limits with priority queueing, and
emits Phoenix spans annotated with cache savings.
"""

__version__ = "0.1.0"
