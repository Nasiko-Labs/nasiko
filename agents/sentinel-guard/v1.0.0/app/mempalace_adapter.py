"""
MemPalace adapter for Sentinel Guard semantic caching.
Uses MemPalace wings to scope cache per agent and provides
graceful degradation if MemPalace is not installed or configured.
"""

import json
import logging
import os
import time
from typing import Any, Optional

logger = logging.getLogger("sentinel.mempalace")


class MemPalaceAdapter:
    """Adapter wrapping MemPalace's search/mine API for use as a cache backend."""

    def __init__(self) -> None:
        self._available = False
        self._palace_path = os.path.expanduser("~/.mempalace/sentinel-cache")
        try:
            from mempalace.config import MempalaceConfig
            from mempalace.searcher import search_memories

            self._search_fn = search_memories
            self._available = True
            logger.info(f"MemPalace adapter ready (palace path: {self._palace_path})")
        except ImportError:
            logger.info("mempalace package not installed – adapter disabled")
        except Exception as exc:
            logger.warning(f"MemPalace init error: {exc}")

    @property
    def available(self) -> bool:
        return self._available

    def search(self, query: str, agent: str, threshold: float = 0.92) -> Optional[dict]:
        """Search for a semantically similar cached response."""
        if not self._available:
            return None
        try:
            results = self._search_fn(
                query=query,
                wing=f"agent_{agent}",
                room="cache",
                n_results=1,
            )
            if results and results.get("results"):
                top = results["results"][0]
                sim = top.get("similarity", 0)
                if sim >= threshold:
                    text = top.get("text", "")
                    # Try to parse as JSON (we store JSON payloads)
                    try:
                        parsed = json.loads(text)
                        parsed["_similarity"] = sim
                        parsed["_cache_source"] = "mempalace"
                        return parsed
                    except (json.JSONDecodeError, TypeError):
                        return {
                            "result": text,
                            "_similarity": sim,
                            "_cache_source": "mempalace",
                        }
            return None
        except Exception as exc:
            logger.debug(f"MemPalace search error: {exc}")
            return None

    def store(self, query: str, response: Any, agent: str) -> None:
        """Store a query-response pair as a MemPalace drawer."""
        if not self._available:
            return
        try:
            from mempalace.miner import mine_text

            payload = json.dumps(response) if not isinstance(response, str) else response
            combined = f"Query: {query}\n\nResponse: {payload}"
            mine_text(
                text=combined,
                wing=f"agent_{agent}",
                room="cache",
                source=f"sentinel-cache-{int(time.time())}",
            )
        except ImportError:
            logger.debug("mempalace.miner not available")
        except Exception as exc:
            logger.debug(f"MemPalace store error: {exc}")

    def flush(self, agent: Optional[str] = None) -> int:
        """Flush MemPalace cache entries. Returns count of flushed items."""
        # MemPalace doesn't have a direct "delete" API – we track this
        # as a no-op for now and rely on TTL/manual cleanup
        logger.info(f"MemPalace flush requested (agent={agent}) – manual cleanup needed")
        return 0
