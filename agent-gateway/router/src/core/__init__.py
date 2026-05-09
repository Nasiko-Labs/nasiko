"""
Core services module for the router application.

Heavy ML dependencies (langchain, FAISS) are imported lazily so that
lightweight modules like cache_service and rate_limiter can be imported
in tests without requiring the full ML stack.
"""

from .agent_registry import AgentRegistry, AgentRegistryError
from .agent_client import AgentClient, AgentClientError
from .session_history import SessionHistoryService, SessionHistoryError
from .cache_service import CacheService
from .rate_limiter import RateLimiter, RateLimitExceeded


def _get_vector_store():
    """Lazy import to avoid pulling in langchain/FAISS at module load time."""
    from .vector_store import VectorStoreService, VectorStoreError
    return VectorStoreService, VectorStoreError


# Keep these importable at the top level for code that already uses them,
# but defer the actual import until first access.
class _LazyVectorStore:
    _VectorStoreService = None
    _VectorStoreError = None

    @classmethod
    def _load(cls):
        if cls._VectorStoreService is None:
            cls._VectorStoreService, cls._VectorStoreError = _get_vector_store()

    def __getattr__(self, name):
        self._load()
        if name == "VectorStoreService":
            return self._VectorStoreService
        if name == "VectorStoreError":
            return self._VectorStoreError
        raise AttributeError(name)


# Direct imports for code that uses `from router.src.core import VectorStoreService`
# These will trigger the lazy load on first use.
try:
    from .vector_store import VectorStoreService, VectorStoreError
except ImportError:
    VectorStoreService = None  # type: ignore
    VectorStoreError = None    # type: ignore


__all__ = [
    "AgentRegistry",
    "AgentRegistryError",
    "VectorStoreService",
    "VectorStoreError",
    "AgentClient",
    "AgentClientError",
    "SessionHistoryService",
    "SessionHistoryError",
    "CacheService",
    "RateLimiter",
    "RateLimitExceeded",
]
