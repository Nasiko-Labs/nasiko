"""A2A face of the copilot: plain-text commands in, JSON projections out.

Commands (case-insensitive):
  contribute to <owner/repo>
  status <thread_id>
  approvals
  approve|reject <G1|G2|G3> <thread_id> [note]
"""

import json
import logging
import re
import uuid

from a2a.helpers import new_task_from_user_message
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.types import (
    Artifact,
    Part,
    TaskArtifactUpdateEvent,
    TaskState,
    TaskStatus,
    TaskStatusUpdateEvent,
)

from copilot import Copilot, GateError, PolicyError

logger = logging.getLogger(__name__)

HELP = (
    "Commands: 'contribute to owner/repo' · 'status t_…' · 'approvals' · "
    "'approve G1 t_… [note]' / 'reject G2 t_… [note]'"
)


class CopilotAgentExecutor(AgentExecutor):
    def __init__(self, copilot: Copilot, allow_a2a_approvals: bool = False):
        self.cp = copilot
        # Approving over A2A lets another agent release a gate; off by default.
        self.allow_a2a_approvals = allow_a2a_approvals

    def handle(self, text: str) -> str:
        s = text.strip()
        try:
            if m := re.search(r"contribute(?:\s+to)?\s+(?:https://github\.com/)?([\w.-]+/[\w.-]+)", s, re.I):
                return json.dumps(self.cp.contribute(m.group(1)))
            if m := re.search(r"status\s+(t_[0-9a-f]+)", s, re.I):
                return json.dumps(self.cp.status(m.group(1)))
            if re.search(r"\bapprovals?\b|\bpending\b", s, re.I) and not re.match(r"\s*(approve|reject)\s", s, re.I):
                return json.dumps({"approvals": self.cp.approvals()})
            if m := re.match(r"\s*(approve|reject)\s+(G[1-3])\s+(t_[0-9a-f]+)\s*(.*)", s, re.I):
                if not self.allow_a2a_approvals:
                    return "Refused: gate decisions are only accepted from a human via POST /a2a/approve."
                return json.dumps(self.cp.decide(m.group(2).upper(), m.group(1), m.group(4),
                                                 m.group(3), approver="a2a-caller"))
        except (PolicyError, GateError, KeyError) as e:
            return f"Refused: {e}"
        return HELP

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        task = context.current_task or new_task_from_user_message(context.message)
        await event_queue.enqueue_event(task)
        reply = self.handle(context.get_user_input())
        await event_queue.enqueue_event(
            TaskArtifactUpdateEvent(
                task_id=task.id,
                context_id=task.context_id,
                artifact=Artifact(artifact_id=str(uuid.uuid4()), parts=[Part(text=reply)]),
                append=False,
                last_chunk=True,
            )
        )
        await event_queue.enqueue_event(
            TaskStatusUpdateEvent(
                task_id=task.id,
                context_id=task.context_id,
                status=TaskStatus(state=TaskState.TASK_STATE_COMPLETED),
            )
        )

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        pass
