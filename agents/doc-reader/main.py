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


def extract_files_from_message(message) -> list[dict]:
    """Extract file parts from an A2A message.

    Handles both the typed Part model (PartContent.raw) and the raw dict
    format forwarded by the orchestrator/proxy.
    """
    files = []
    if not message or not hasattr(message, "parts"):
        return files

    for part in message.parts:
        # Typed Part model: part.content is PartContent with .raw bytes
        content = getattr(part, "content", None)
        if content is not None:
            raw = getattr(content, "raw", None)
            if raw is not None:
                files.append({
                    "filename": getattr(part, "filename", None) or "unnamed",
                    "media_type": getattr(part, "media_type", None) or "application/octet-stream",
                    "data": raw if isinstance(raw, bytes) else base64.b64decode(raw),
                })
                continue

        # Raw dict forwarded by the orchestrator JSON body
        raw_part = part if isinstance(part, dict) else (
            part.model_dump() if hasattr(part, "model_dump") else None
        )
        if not raw_part:
            continue

        if "raw" in raw_part:
            raw_b64 = raw_part["raw"]
            files.append({
                "filename": raw_part.get("filename") or "unnamed",
                "media_type": raw_part.get("mediaType") or "application/octet-stream",
                "data": base64.b64decode(raw_b64) if isinstance(raw_b64, str) else raw_b64,
            })
        elif "file" in raw_part:
            fd = raw_part["file"]
            files.append({
                "filename": fd.get("name") or "unnamed",
                "media_type": fd.get("mimeType") or "application/octet-stream",
                "data": base64.b64decode(fd["bytes"]) if "bytes" in fd else b"",
            })

    return files


class DocReaderExecutor(AgentExecutor):
    def __init__(self, llm: AsyncOpenAI, model: str):
        self.llm = llm
        self.model = model

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input() or ""
        task = context.current_task or new_task_from_user_message(context.message)
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

        file_descriptions = []
        for f in files:
            try:
                text = f["data"].decode("utf-8", errors="replace")
            except Exception:
                text = f"[Binary file, {len(f['data'])} bytes]"

            # Truncate very large files to stay within token limits
            max_chars = 50_000
            if len(text) > max_chars:
                text = text[:max_chars] + f"\n\n... [truncated, {len(text)} total chars]"

            file_descriptions.append(
                f"--- File: {f['filename']} ({f['media_type']}) ---\n{text}"
            )

        files_context = "\n\n".join(file_descriptions)
        prompt = query if query else "Summarize the uploaded document(s)."

        return await self._chat_with_context(prompt, files_context)

    async def _chat_with_context(self, query: str, file_context: str) -> str:
        resp = await self.llm.chat.completions.create(
            model=self.model,
            messages=[
                {"role": "system", "content": (
                    "You are a document analysis assistant. The user has uploaded one or more "
                    "files whose contents are provided below. Answer the user's question about "
                    "the documents. If no specific question is asked, provide a concise summary.\n\n"
                    f"## Uploaded Documents\n\n{file_context}"
                )},
                {"role": "user", "content": query},
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
        description="Reads uploaded documents (text, CSV, JSON, code, logs) and answers questions about them.",
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
                description="Upload a file and ask questions about it. Supports text, CSV, JSON, code, logs, and more.",
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