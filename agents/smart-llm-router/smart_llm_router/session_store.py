"""Session datastore — running savings counters keyed by session_id."""

from __future__ import annotations

from dataclasses import dataclass
from threading import Lock


@dataclass
class SessionTotals:
    running_saved_usd: float = 0.0
    running_calls: int = 0
    running_cache_hits: int = 0


class SessionStore:
    """In-memory stand-in for the DronaHQ datastore."""

    def __init__(self) -> None:
        self._lock = Lock()
        self._sessions: dict[str, SessionTotals] = {}

    def get(self, session_id: str) -> SessionTotals:
        with self._lock:
            return self._sessions.setdefault(session_id, SessionTotals())

    def record(self, session_id: str, saved_usd: float, cache_hit: bool) -> SessionTotals:
        with self._lock:
            t = self._sessions.setdefault(session_id, SessionTotals())
            t.running_saved_usd += max(0.0, saved_usd)
            t.running_calls += 1
            if cache_hit:
                t.running_cache_hits += 1
            return SessionTotals(
                running_saved_usd=t.running_saved_usd,
                running_calls=t.running_calls,
                running_cache_hits=t.running_cache_hits,
            )

    def reset(self, session_id: str | None = None) -> None:
        with self._lock:
            if session_id is None:
                self._sessions.clear()
            else:
                self._sessions.pop(session_id, None)


STORE = SessionStore()
