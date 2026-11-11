"""Document reader agent — accepts file uploads and answers questions about them."""
import base64
import logging
import os

import click
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
from openai import AsyncOpenAI
from opentelemetry import context as otel_context, trace
from starlette.applications import Starlette

from telemetry import TraceparentMiddleware, init_telemetry, request_otel_context

logging.basicConfig(level=logging.INFO)
init_telemetry(os.environ.get("OTEL_SERVICE_NAME", "doc-reader-agent"))
logger = logging.getLogger(__name__)


IMAGE_TYPES = {"image/png", "image/jpeg", "image/gif", "image/webp"}
TEXT_DOCUMENT_TYPES = {"application/json", "application/xml", "application/yaml", "application/x-yaml"}
MAX_TEXT_CHARS = 50_000


def extract_files_from_message(message) -> list[dict]:
    if message is None:
        return []
    return [
        {
            "filename": part.filename or "unnamed",
            "media_type": (part.media_type or "application/octet-stream").split(";")[0].strip().lower(),
            "data": part.raw,
        }
        for part in message.parts
        if part.WhichOneof("content") == "raw"
    ]


def file_content_part(f: dict) -> dict:
    name, media_type, data = f["filename"], f["media_type"], f["data"]
    if media_type in IMAGE_TYPES:
        encoded = base64.b64encode(data).decode()
        return {"type": "image_url", "image_url": {"url": f"data:{media_type};base64,{encoded}"}}
    if media_type == "application/pdf":
        encoded = base64.b64encode(data).decode()
        return {
            "type": "file",
            "file": {"filename": name, "file_data": f"data:application/pdf;base64,{encoded}"},
        }
    if media_type.startswith("text/") or media_type in TEXT_DOCUMENT_TYPES:
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            return {
                "type": "text",
                "text": f"--- File: {name} ({media_type}) was not read: it is not valid UTF-8 text ---",
            }
        if len(text) > MAX_TEXT_CHARS:
            text = text[:MAX_TEXT_CHARS] + f"\n\n... [truncated, {len(text)} total chars]"
        return {"type": "text", "text": f"--- File: {name} ({media_type}) ---\n{text}"}
    return {
        "type": "text",
        "text": f"--- File: {name} ({media_type}) was not read: this agent reads images "
        "(PNG, JPEG, GIF, WebP), PDFs and text files ---",
    }


class DocReaderExecutor(AgentExecutor):
    def __init__(self, llm: AsyncOpenAI, model: str):
        self.llm = llm
        self.model = model

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input() or ""
        task = context.current_task
        if task is None:
            # Enqueue only a new task. Re-enqueueing a stored input-required task on resume makes
            # the SDK replay the stale pause as the first event and drop the human's answer.
            task = new_task_from_user_message(context.message)
            await event_queue.enqueue_event(task)

        tracer = trace.get_tracer("doc-reader-agent")
        parent_ctx = request_otel_context.get() or otel_context.Context()
        with tracer.start_as_current_span("doc-reader.request", context=parent_ctx) as span:
            span.set_attribute("session.id", task.context_id)

            await event_queue.enqueue_event(
                new_text_status_update_event(
                    task_id=task.id, context_id=task.context_id,
                    state=TaskState.TASK_STATE_WORKING, text="Processing...",
                )
            )

            try:
                files = extract_files_from_message(context.message)
                result = await self._process(query, files)

                await event_queue.enqueue_event(
                    new_text_artifact_update_event(
                        task_id=task.id, context_id=task.context_id,
                        name="response", text=result,
                    )
                )
            except Exception as e:
                logger.error(f"Error: {e}", exc_info=True)
                span.record_exception(e)
                await event_queue.enqueue_event(
                    new_text_status_update_event(
                        task_id=task.id, context_id=task.context_id,
                        state=TaskState.TASK_STATE_FAILED, text=f"Error: {e}",
                    )
                )
                return

            await event_queue.enqueue_event(
                new_text_status_update_event(
                    task_id=task.id, context_id=task.context_id,
                    state=TaskState.TASK_STATE_COMPLETED, text="Done",
                )
            )

    async def _process(self, query: str, files: list[dict]) -> str:
        if not files:
            return await self._chat(
                query or "No files were uploaded. Please upload a document to analyze."
            )

        prompt = query if query else "Summarize the uploaded document(s)."
        content = [{"type": "text", "text": prompt}, *(file_content_part(f) for f in files)]
        resp = await self.llm.chat.completions.create(
            model=self.model,
            messages=[
                {"role": "system", "content": (
                    "You are a document analysis assistant. The user has attached one or more "
                    "files to their message. Answer the user's question about them. If no "
                    "specific question is asked, provide a concise summary."
                )},
                {"role": "user", "content": content},
            ],
        )
        return resp.choices[0].message.content

    async def _chat(self, query: str) -> str:
        resp = await self.llm.chat.completions.create(
            model=self.model,
            messages=[
                {"role": "system", "content": "You are a document analysis assistant."},
                {"role": "user", "content": query},
            ],
        )
        return resp.choices[0].message.content

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        pass


@click.command()
@click.option("--host", default="0.0.0.0")
@click.option("--port", default=int(os.environ.get("PORT", "8000")), type=int)
def main(host, port):
    llm = AsyncOpenAI(
        api_key=os.environ.get("OPENAI_API_KEY"),
        base_url=os.environ.get("OPENAI_BASE_URL"),
    )
    model = os.environ.get("MODEL", os.environ.get("OPENAI_MODEL", "gpt-4o-mini"))

    agent_card = AgentCard(
        name="Doc Reader",
        description="Reads uploaded documents (PDFs, images, text, CSV, JSON, code, logs) and answers questions about them.",
        supported_interfaces=[
            AgentInterface(protocol_binding="JSONRPC", url=f"http://{host}:{port}/"),
        ],
        version="1.0.0",
        default_input_modes=["text/plain", "application/octet-stream"],
        default_output_modes=["text/plain"],
        capabilities=AgentCapabilities(streaming=False),
        skills=[
            AgentSkill(
                id="doc-reader",
                name="Document Reader",
                description="Upload a file and ask questions about it. Supports PDFs, images, text, CSV, JSON, code and logs.",
                tags=["documents", "files", "upload", "summarize", "analyze"],
                examples=[
                    "Summarize this document",
                    "What are the key findings in this report?",
                    "Extract all email addresses from this file",
                    "Explain this code file",
                ],
            )
        ],
    )

    handler = DefaultRequestHandler(
        agent_executor=DocReaderExecutor(llm, model),
        # Deliberately in memory: uploaded documents travel inside the task's message history, so a
        # task body routinely exceeds the platform state API's per-value limit (1 MiB by default).
        # Move to nasiko_state.make_task_store() once the state API supports large bodies.
        task_store=InMemoryTaskStore(),
        agent_card=agent_card,
    )

    routes = []
    routes.extend(create_agent_card_routes(agent_card))
    routes.extend(create_jsonrpc_routes(handler, rpc_url="/"))

    app = Starlette(routes=routes)
    app = TraceparentMiddleware(app)
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()