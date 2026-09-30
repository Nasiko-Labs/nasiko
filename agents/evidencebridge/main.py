"""EvidenceBridge Agent — verify suppliers via the EvidenceBridge backend."""
import logging
import os
import re
import asyncio

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

try:
    from telemetry import init_telemetry
    init_telemetry()
except ImportError:
    logger.warning("telemetry.py not found — OTel telemetry disabled")

import click
import httpx
import uvicorn
from a2a.helpers import (
    new_task_from_user_message,
    new_text_artifact_update_event,
    new_text_status_update_event,
)
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.routes import create_agent_card_routes, create_jsonrpc_routes
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import (
    AgentCapabilities,
    AgentCard,
    AgentInterface,
    AgentSkill,
    TaskState,
)
from starlette.applications import Starlette


def extract_supplier_name(query: str) -> str | None:
    query = query.strip()
    match = re.search(r'(?i)^verify\s+(?:supplier\s+)?(.+)', query)
    if match:
        name = match.group(1).strip()
    else:
        name = query
        
    name = re.sub(r'[.!?]$', '', name).strip()
    
    if len(name) > 1:
        return name
    return None


def format_response(data: dict) -> str:
    supplier_name = data.get("supplier_name", "Unknown Supplier")
    
    report = data.get("report", {})
    summary = report.get("summary", {})
    findings = report.get("findings", [])
    recommendations = report.get("recommended_actions", [])
    
    evidence_urls = set()
    for e in data.get("evidence", []):
        if url := e.get("source_url"):
            evidence_urls.add(url)
            
    lines = [f"**Supplier Verification Report: {supplier_name}**"]
    
    if summary_text := summary.get("summary_text"):
        lines.append(f"\n**Summary:**\n{summary_text}")
        
    if findings:
        lines.append("\n**Findings:**")
        for f in findings:
            field = f.get("field", "Unknown")
            status = f.get("status", "NEEDS_VERIFICATION")
            explanation = f.get("explanation", "")
            lines.append(f"- **{field}** ({status}): {explanation}")
            
    if evidence_urls:
        lines.append("\n**Evidence / Source URLs:**")
        for url in sorted(evidence_urls):
            lines.append(f"- {url}")
            
    if recommendations:
        lines.append("\n**Recommended Verification Actions:**")
        for r in recommendations:
            lines.append(f"- {r}")
            
    return "\n".join(lines)


class EvidenceBridgeExecutor(AgentExecutor):
    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input()
        task = context.current_task or new_task_from_user_message(context.message)
        await event_queue.enqueue_event(task)

        api_url = os.environ.get("EVIDENCEBRIDGE_API_URL")
        if not api_url:
            msg = "Error: EVIDENCEBRIDGE_API_URL environment variable is missing."
            logger.error(msg)
            await event_queue.enqueue_event(new_text_status_update_event(task.id, task.context_id, TaskState.TASK_STATE_FAILED, msg))
            return

        supplier_name = extract_supplier_name(query)
        if not supplier_name:
            msg = "Could not extract a supplier name from the request."
            await event_queue.enqueue_event(new_text_status_update_event(task.id, task.context_id, TaskState.TASK_STATE_FAILED, msg))
            return

        await event_queue.enqueue_event(
            new_text_status_update_event(
                task_id=task.id,
                context_id=task.context_id,
                state=TaskState.TASK_STATE_WORKING,
                text=f"Verifying supplier '{supplier_name}' via EvidenceBridge...",
            )
        )

        try:
            async with httpx.AsyncClient(timeout=15.0) as client:
                resp = await client.post(
                    api_url,
                    json={"supplier_name": supplier_name}
                )
                resp.raise_for_status()
                data = resp.json()
        except Exception as e:
            error_msg = f"Failed to contact EvidenceBridge backend: {e}"
            logger.error(error_msg)
            await event_queue.enqueue_event(
                new_text_status_update_event(
                    task_id=task.id,
                    context_id=task.context_id,
                    state=TaskState.TASK_STATE_FAILED,
                    text=error_msg,
                )
            )
            return

        formatted_response = format_response(data)

        chunk_size = 100
        for i in range(0, len(formatted_response), chunk_size):
            chunk = formatted_response[i:i+chunk_size]
            await event_queue.enqueue_event(
                new_text_artifact_update_event(
                    task_id=task.id,
                    context_id=task.context_id,
                    name="verification-result",
                    text=chunk,
                )
            )
            await asyncio.sleep(0.01)

        await event_queue.enqueue_event(
            new_text_status_update_event(
                task_id=task.id,
                context_id=task.context_id,
                state=TaskState.TASK_STATE_COMPLETED,
                text="Verification complete.",
            )
        )

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        pass


@click.command()
@click.option("--host", default="0.0.0.0")
@click.option("--port", default=int(os.environ.get("PORT", "8000")), type=int)
def main(host, port):
    if not os.environ.get("EVIDENCEBRIDGE_API_URL"):
        raise RuntimeError("EVIDENCEBRIDGE_API_URL environment variable is required and must not be empty.")

    agent_card = AgentCard(
        name="EvidenceBridge",
        description="Supplier claim verification agent that researches public evidence and returns traceable verification findings before payment.",
        supported_interfaces=[
            AgentInterface(protocol_binding="JSONRPC", protocol_version="1.0", url=f"http://{host}:{port}/"),
        ],
        version="1.0.0",
        default_input_modes=["text/plain"],
        default_output_modes=["text/plain"],
        capabilities=AgentCapabilities(streaming=True),
        skills=[
            AgentSkill(
                id="verify_supplier",
                name="Verify Supplier",
                description="supplier verification, evidence research, claim comparison, procurement verification",
                tags=["verification", "procurement", "evidence", "claims"],
                examples=["Verify supplier Acme Technologies"],
            )
        ],
    )

    handler = DefaultRequestHandler(
        agent_executor=EvidenceBridgeExecutor(),
        task_store=InMemoryTaskStore(),
        agent_card=agent_card,
    )

    routes = []
    routes.extend(create_agent_card_routes(agent_card))
    routes.extend(create_jsonrpc_routes(handler, rpc_url="/"))

    app = Starlette(routes=routes)
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()
