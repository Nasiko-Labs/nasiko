"""
MemPalace adapter for Sentinel Guard semantic caching.
Uses MemPalace wings to scope cache per agent.

MemPalace stores verbatim text in a structured palace
(wings -> rooms -> drawers) with ChromaDB-backed vector search.
Each agent gets its own wing, and cached responses are stored as drawers.
"""

import json
import logging
import os
import time
from typing import Any, Optional

logger = logging.getLogger("sentinel.mempalace")


class MemPalaceAdapter:
    """Adapter wrapping MemPalace's add_drawer/search_memories API for caching."""

    def __init__(self) -> None:
        self._available = False
        self._collection = None
        self._palace_path = None

        try:
            from mempalace.config import MempalaceConfig
            from mempalace.miner import get_collection, add_drawer
            from mempalace.searcher import search_memories

            # Store function references
            self._search_fn = search_memories
            self._add_drawer_fn = add_drawer

            # Initialize palace path and collection
            config = MempalaceConfig()
            self._palace_path = config.palace_path
            os.makedirs(self._palace_path, exist_ok=True)

            # Get the ChromaDB collection (creates if needed)
            self._collection = get_collection(self._palace_path)

            self._available = True
            logger.info(
                f"MemPalace adapter ready "
                f"(palace: {self._palace_path})"
            )
        except ImportError:
            logger.warning("mempalace package not installed -- adapter disabled")
        except Exception as exc:
            logger.warning(f"MemPalace init error: {exc}")

    @property
    def available(self) -> bool:
        return self._available

    def search(self, query: str, agent: str, threshold: float = 0.55) -> Optional[dict]:
        """
        Search for a semantically similar cached response in MemPalace.

        MemPalace similarity scores are lower than raw cosine similarity
        because drawers store combined "Query: ...\nResponse: ..." text,
        which dilutes the embedding match. A threshold of ~0.55 is
        appropriate for L3 (compared to 0.92 for L2's pure-query embeddings).
        """
        if not self._available:
            return None
        try:
            results = self._search_fn(
                query=query,
                palace_path=self._palace_path,
                wing=f"agent_{agent}",
                room="cache",
                n_results=1,
            )

            if results and results.get("results"):
                top = results["results"][0]
                # MemPalace returns 'similarity' (already 1 - distance)
                sim = top.get("similarity", 0)

                if sim >= threshold:
                    text = top.get("text", "")
                    # Our drawers store JSON: "Query: ...\n\nResponse: ..."
                    # Extract the response part
                    response_text = text
                    if "\n\nResponse: " in text:
                        response_text = text.split("\n\nResponse: ", 1)[1]

                    # Try to parse as JSON
                    try:
                        parsed = json.loads(response_text)
                        parsed["_similarity"] = sim
                        parsed["_cache_source"] = "mempalace"
                        return parsed
                    except (json.JSONDecodeError, TypeError):
                        return {
                            "result": response_text,
                            "_similarity": sim,
                            "_cache_source": "mempalace",
                        }

            return None
        except Exception as exc:
            logger.debug(f"MemPalace search error: {exc}")
            return None

    def store(self, query: str, response: Any, agent: str) -> None:
        """Store a query-response pair as a MemPalace drawer."""
        if not self._available or not self._collection:
            return
        try:
            payload = json.dumps(response) if not isinstance(response, str) else response
            combined = f"Query: {query}\n\nResponse: {payload}"

            self._add_drawer_fn(
                collection=self._collection,
                wing=f"agent_{agent}",
                room="cache",
                content=combined,
                source_file=f"sentinel-cache-{int(time.time())}",
                chunk_index=0,
                agent="sentinel-guard",
            )
            logger.debug(f"MemPalace stored drawer: agent={agent}, query={query[:50]}")
        except Exception as exc:
            logger.warning(f"MemPalace store error: {exc}")

    def flush(self, agent: Optional[str] = None) -> int:
        """
        Flush MemPalace cache entries.
        Note: MemPalace doesn't have a bulk-delete API,
        so we log the request. Entries will naturally be
        superseded by newer ones on search.
        """
        logger.info(f"MemPalace flush requested (agent={agent})")
        return 0

    def stats(self) -> dict:
        """Return MemPalace backend statistics."""
        if not self._available:
            return {"available": False}

        try:
            count = self._collection.count() if self._collection else 0
            return {
                "available": True,
                "palace_path": self._palace_path,
                "total_drawers": count,
            }
        except Exception:
            return {"available": True, "error": "stats unavailable"}
