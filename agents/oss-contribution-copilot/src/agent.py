"""Thin client onto the OSS Contribution Copilot's own REST API.

This is the whole adapter's reason for being narrow: every method here maps
one A2A/REST-façade capability onto exactly the copilot's existing FastAPI
routes (``app/api/routes.py``). It never talks to GitHub, never resumes a
graph with a decision it invented, and never decides which issue to work —
that is what ``architecture.md``'s "the adapter adds no capability" line
means in code.

Reads:  the copilot's HTTP API only (``COPILOT_URL``, default
        ``http://127.0.0.1:8000``).
Writes: nothing directly — every write is the copilot's own route accepting
        a human's (or DronaHQ's forwarded) instruction.
"""

from __future__ import annotations

import logging
import os
from typing import Any

import httpx

logger = logging.getLogger(__name__)

#: Gate id, as the copilot's graph names it -> the G-number the hackathon
#: projection (architecture.md §2.5) and the demo script use on screen.
GATE_LABELS: dict[str, str] = {
    "claim": "G1",
    "plan": "G2",
    "pr": "G3",
    "reply": "G4",
}

#: Fallback choices if a payload is ever missing "options" (it never should —
#: every gate builder sets both "question" and "options" — but a fallback
#: keeps the adapter from crashing on a shape it does not own).
_DEFAULT_CHOICES = ("approve", "reject")

#: The projection shows a gate's *label* ("G1"); a caller may hand either that
#: label or the copilot's own internal gate name ("claim") back on
#: `decide_approval` — DronaHQ round-trips whatever the gate object carried.
#: Accept both so a decision never fails on a naming mismatch that is entirely
#: this adapter's own doing.
_REVERSE_GATE_LABELS: dict[str, str] = {v: k for k, v in GATE_LABELS.items()}


class CopilotError(Exception):
    """Wraps a failed call to the copilot's API — the executor shows this verbatim."""

    def __init__(self, status_code: int, detail: str) -> None:
        super().__init__(detail)
        self.status_code = status_code
        self.detail = detail


class CopilotClient:
    """Forwards to the copilot's REST API and projects its state (§2.5)."""

    def __init__(self, base_url: str | None = None, detail_base_url: str | None = None) -> None:
        self.base_url = (base_url or os.getenv("COPILOT_URL", "http://127.0.0.1:8000")).rstrip("/")
        #: Mission Control's own base URL, for the projection's `detail_url`.
        self.detail_base_url = (
            detail_base_url or os.getenv("MISSION_CONTROL_URL", "http://localhost:5173")
        ).rstrip("/")

    async def _request(self, method: str, path: str, **kwargs: Any) -> Any:
        async with httpx.AsyncClient(base_url=self.base_url, timeout=30.0) as client:
            response = await client.request(method, path, **kwargs)
        if response.status_code >= 400:
            detail = response.text
            try:
                detail = response.json().get("detail", detail)
            except Exception:
                pass
            raise CopilotError(response.status_code, str(detail))
        if not response.content:
            return {}
        return response.json()

    # -- the four skills ----------------------------------------------------

    async def start_contribution(self, repo: str) -> dict[str, Any]:
        """Create a session and start ingesting `repo` (§2.3 `contribute`).

        This is exactly what Mission Control's own "contribute" action does —
        create a session, then ``POST /api/pick-repo`` — nothing more. Which
        issue gets worked, and every later phase transition, stays a human (or
        the copilot's own graph) decision made through its existing surface,
        not invented here.
        """
        session = await self._request("POST", "/api/sessions", json={"title": f"contribute: {repo}"})
        thread_id = str(session["thread_id"])
        await self._request(
            "POST", "/api/pick-repo", json={"thread_id": thread_id, "full_name": repo}
        )
        result = await self.projection(thread_id)
        await _notify_dronahq(result)
        return result

    async def get_status(self, thread_id: str) -> dict[str, Any]:
        """§2.3 `contribution_status` — the projection, nothing wider."""
        return await self.projection(thread_id)

    async def list_approvals(self) -> list[dict[str, Any]]:
        """§2.3 `list_approvals` — every open gate, across every thread."""
        body = await self._request("GET", "/api/approvals")
        rows = body.get("rows", []) if isinstance(body, dict) else body
        threads = sorted({str(row["thread"]) for row in rows})
        return [await self.projection(thread_id) for thread_id in threads]

    async def decide_approval(
        self, thread_id: str, gate: str, decision: str, note: str = ""
    ) -> dict[str, Any]:
        """§2.3 `decide_approval` — forward a human's decision, verbatim.

        `action` and `edits` are exactly what
        ``POST /api/approvals/{thread}`` (``ApprovalDecision``) already
        accepts; this method invents no new decision field and applies no
        judgement of its own about whether the decision is a good one.
        """
        gate = _REVERSE_GATE_LABELS.get(gate.upper(), gate)
        body: dict[str, Any] = {"gate": gate, "action": decision}
        if note:
            body["edits"] = {f"{gate}_text": note} if gate in ("claim", "reply") else {"note": note}
        await self._request("POST", f"/api/approvals/{thread_id}", json=body)
        result = await self.projection(thread_id)
        if result.get("gate"):
            # The decision just taken may have opened the *next* gate
            # (claim -> plan, plan -> pr) in the same graph run — tell
            # DronaHQ so the human sees it without polling (D4b).
            await _notify_dronahq(result)
        return result

    # -- the projection (§2.5) ----------------------------------------------

    async def projection(self, thread_id: str) -> dict[str, Any]:
        """Build the small, ~200-token view every skill returns.

        Three copilot reads, all existing routes: thread state (for phase and
        repo), that thread's pending gate (if any), and its spend so far. Never
        the raw plan, diff or test output — those stay behind `detail_url`.
        """
        state = await self._request("GET", f"/api/state/{thread_id}")
        values = state.get("state", {})
        repo = values.get("repo") or {}
        repo_slug = f"{repo.get('owner', '')}/{repo.get('name', '')}".strip("/")
        phase = str(values.get("phase", "unknown"))

        approvals = await self._request("GET", f"/api/approvals/{thread_id}")
        pending = approvals[0] if approvals else None

        cost = await self._request("GET", f"/api/costs/{thread_id}")

        gate_obj = None
        headline = f"{repo_slug or thread_id}: {phase}" if repo_slug else f"phase: {phase}"
        next_step = "waiting_on_human" if pending else "in_progress"
        if pending:
            gate = str(pending["gate"])
            payload = pending.get("payload", {})
            gate_obj = {
                "id": GATE_LABELS.get(gate, gate),
                "question": payload.get("question", f"Decide gate {gate!r}?"),
                "choices": list(payload.get("options") or _DEFAULT_CHOICES),
            }
            headline = f"{GATE_LABELS.get(gate, gate)} pending — {gate_obj['question']}"

        return {
            "thread_id": thread_id,
            "repo": repo_slug,
            "phase": phase,
            "headline": headline,
            "gate": gate_obj,
            "next": next_step,
            "cost_usd": round(float(cost.get("total_usd", 0.0)), 4),
            "detail_url": f"{self.detail_base_url}/work/{thread_id}",
        }


async def _notify_dronahq(result: dict[str, Any]) -> None:
    """Tell DronaHQ a gate is open (skills-dronahq.md D4b) — fire-and-forget.

    The gate is the copilot's; this notification is a courtesy. Reads both
    values from the environment on every call (never baked in, never logged)
    so a missing webhook simply means no notification, not a startup crash —
    and a DronaHQ outage can never block a gate.
    """
    url = os.getenv("DRONAHQ_WEBHOOK_URL")
    api_key = os.getenv("DRONAHQ_API_KEY")
    if not url or not api_key:
        return
    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            await client.post(
                url,
                headers={"api-key": api_key},
                json={"thread_id": result["thread_id"], "event": "gate_open", **result},
            )
    except Exception:
        logger.warning("dronahq notify failed; gate is unaffected")


__all__ = ["CopilotClient", "CopilotError", "GATE_LABELS"]
