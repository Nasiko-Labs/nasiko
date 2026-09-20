"""A2A executor for the OSS Contribution Copilot.

Mirrors ``agents/claude-sdk/src/agent_executor.py``'s shape (an
``AgentExecutor`` that reads the incoming message, drives one task to
completion, and reports through a ``TaskUpdater``). The difference is what it
dispatches to: four fixed skills (§2.3), each a direct call into
``agent.CopilotClient`` — never a model call, never a decision of its own.

No business logic lives here. No GitHub write path lives here either — every
write already happened, or will happen, behind the copilot's own gates.
"""

import json
import logging
import re
from typing import Any

from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.server.tasks import TaskUpdater
from a2a.types import InternalError, Part, TaskState, TextPart, UnsupportedOperationError
from a2a.utils import new_agent_text_message, new_task
from a2a.utils.errors import ServerError

from agent import CopilotClient, CopilotError

logger = logging.getLogger(__name__)

_REPO = re.compile(r"([A-Za-z0-9._-]+/[A-Za-z0-9._-]+)")
_THREAD = re.compile(r"(t_[0-9a-fA-F]+)")
_GATE = re.compile(r"\b(claim|plan|pr|reply)\b", re.IGNORECASE)
_DECISION = re.compile(r"\b(approve_without_claim|approve|reject|revise|abort|edit|discard)\b", re.IGNORECASE)


def parse_request(text: str) -> dict[str, Any]:
    """Work out which of the four skills a message means, and its arguments.

    Two input shapes, both deterministic — never a model guess:

    1. Structured JSON, e.g. ``{"skill": "contribute", "repo": "owner/name"}``
       — the shape Nasiko or a careful caller would send.
    2. A short natural-language command matching one of the four verbs, for a
       human typing directly into an A2A client. DronaHQ itself talks to the
       REST façade (§2.6), not this parser — see `skills-dronahq.md`.
    """
    stripped = text.strip()
    if stripped.startswith("{"):
        try:
            payload = json.loads(stripped)
            if isinstance(payload, dict) and payload.get("skill"):
                return payload
        except json.JSONDecodeError:
            pass

    lowered = stripped.lower()
    if "approve" in lowered or "reject" in lowered or "decide" in lowered:
        thread = _THREAD.search(stripped)
        gate = _GATE.search(stripped)
        decision = _DECISION.search(stripped)
        if thread and gate and decision:
            return {
                "skill": "decide_approval",
                "thread_id": thread.group(1),
                "gate": gate.group(1).lower(),
                "decision": decision.group(1).lower(),
            }
    if "list" in lowered and "approval" in lowered or "pending" in lowered and "gate" in lowered:
        return {"skill": "list_approvals"}
    if "status" in lowered:
        thread = _THREAD.search(stripped)
        if thread:
            return {"skill": "contribution_status", "thread_id": thread.group(1)}
    if "contribute" in lowered or "start a contribution" in lowered:
        repo = _REPO.search(stripped)
        if repo:
            return {"skill": "contribute", "repo": repo.group(1)}

    return {"skill": None, "error": f"could not map {text!r} to a known skill"}


class OSSContributionCopilotExecutor(AgentExecutor):
    """Dispatches an A2A message to one of the copilot's four skills."""

    def __init__(self) -> None:
        self.client = CopilotClient()

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input()
        task = context.current_task
        if not task:
            task = new_task(context.message)
            await event_queue.enqueue_event(task)
        updater = TaskUpdater(event_queue, task.id, task.context_id)

        try:
            result = await self._run_skill(query)
            await updater.add_artifact(
                [Part(root=TextPart(text=json.dumps(result)))],
                name="projection",
            )
            await updater.complete()
        except CopilotError as error:
            # Shown verbatim (skills-dronahq.md D1): never retried, never
            # papered over with an invented workaround.
            await updater.update_status(
                TaskState.failed,
                new_agent_text_message(f"copilot error {error.status_code}: {error.detail}", task.context_id, task.id),
                final=True,
            )
        except Exception as e:
            logger.error(f"Error: {e}")
            raise ServerError(error=InternalError()) from e

    async def _run_skill(self, query: str) -> dict[str, Any]:
        request = parse_request(query)
        skill = request.get("skill")

        if skill == "contribute":
            return await self.client.start_contribution(str(request["repo"]))
        if skill == "contribution_status":
            return await self.client.get_status(str(request["thread_id"]))
        if skill == "list_approvals":
            return {"approvals": await self.client.list_approvals()}
        if skill == "decide_approval":
            return await self.client.decide_approval(
                thread_id=str(request["thread_id"]),
                gate=str(request["gate"]),
                decision=str(request["decision"]),
                note=str(request.get("note", "")),
            )
        raise CopilotError(422, request.get("error", "unrecognised request"))

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        raise ServerError(error=UnsupportedOperationError())
