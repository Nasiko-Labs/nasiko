"""Assistant agent — discovers agents via A2A registry and delegates tasks."""
import json
import logging
import os
import uuid
from collections.abc import AsyncIterator

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
from openai import AsyncOpenAI
from starlette.applications import Starlette

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

DISCOVERY_URL = os.environ.get("A2A_DISCOVERY_URL", "")


class AssistantExecutor(AgentExecutor):
    def __init__(self, llm: AsyncOpenAI, model: str):
        self.llm = llm
        self.model = model

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        query = context.get_user_input()

        task = context.current_task or new_task_from_user_message(context.message)
        await event_queue.enqueue_event(task)

        await event_queue.enqueue_event(
            new_text_status_update_event(
                task_id=task.id,
                context_id=task.context_id,
                state=TaskState.TASK_STATE_WORKING,
                text="Discovering agents...",
            )
        )

        try:
            if DISCOVERY_URL:
                agents = await self._discover_agents()
                plan = await self._plan(query, agents)
                results = await self._delegate(plan, query)
            else:
                agents = []
                results = []

            full_response = ""
            async for chunk in self._synthesize(query, results):
                full_response += chunk

            await event_queue.enqueue_event(
                new_text_artifact_update_event(
                    task_id=task.id,
                    context_id=task.context_id,
                    name="assistant-response",
                    text=full_response,
                )
            )

        except Exception as e:
            logger.error(f"Orchestration error: {e}")
            await event_queue.enqueue_event(
                new_text_status_update_event(
                    task_id=task.id,
                    context_id=task.context_id,
                    state=TaskState.TASK_STATE_FAILED,
                    text=f"Error: {e}",
                )
            )
            return

        await event_queue.enqueue_event(
            new_text_status_update_event(
                task_id=task.id,
                context_id=task.context_id,
                state=TaskState.TASK_STATE_COMPLETED,
                text=full_response,
            )
        )

    async def _discover_agents(self) -> list[dict]:
        async with httpx.AsyncClient(timeout=10.0) as client:
            resp = await client.post(
                f"{DISCOVERY_URL.rstrip('/')}/a2a/v1",
                json={
                    "jsonrpc": "2.0",
                    "id": str(uuid.uuid4()),
                    "method": "message/send",
                    "params": {
                        "message": {
                            "messageId": str(uuid.uuid4()),
                            "role": "user",
                            "parts": [{"kind": "text", "text": ""}],
                        }
                    },
                },
            )
            data = resp.json()
            result = data.get("result", {})
            agents = []
            for artifact in result.get("artifacts", []):
                for part in artifact.get("parts", []):
                    if part.get("kind") == "data":
                        agents = part.get("data", {}).get("agents", [])

            for agent in agents:
                url = agent.get("url", "")
                if "localhost" in url:
                    agent["url"] = url.replace("localhost", "host.docker.internal")

            return [a for a in agents if a.get("name") != "assistant-agent"]

    async def _plan(self, query: str, agents: list[dict]) -> list[dict]:
        if not agents:
            return []

        agent_list = "\n".join(
            f"- name: {a.get('name')}, description: {a.get('description', 'none')}, url: {a.get('url', '')}"
            for a in agents
        )

        resp = await self.llm.chat.completions.create(
            model=self.model,
            messages=[
                {"role": "system", "content": f"""You are a routing agent. Given a user query and available agents, decide which to call.

Available agents:
{agent_list}

Respond ONLY with a JSON array: [{{"name": "...", "url": "...", "sub_query": "..."}}]
If no agent fits, respond with []. Do NOT delegate to yourself (Assistant)."""},
                {"role": "user", "content": query},
            ],
        )

        text = resp.choices[0].message.content.strip()
        if text.startswith("```"):
            text = text.split("\n", 1)[1].rsplit("```", 1)[0].strip()
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            logger.warning(f"Bad plan JSON: {text}")
            return []

    async def _delegate(self, plan: list[dict], original_query: str) -> list[dict]:
        results = []
        async with httpx.AsyncClient(timeout=30.0) as client:
            for item in plan:
                url = item.get("url", "").rstrip("/")
                sub_query = item.get("sub_query", original_query)
                name = item.get("name", "unknown")

                if not url:
                    continue

                try:
                    resp = await client.post(
                        f"{url}/",
                        json={
                            "jsonrpc": "2.0",
                            "id": str(uuid.uuid4()),
                            "method": "message/send",
                            "params": {
                                "message": {
                                    "messageId": str(uuid.uuid4()),
                                    "role": "user",
                                    "parts": [{"kind": "text", "text": sub_query}],
                                }
                            },
                        },
                    )
                    text = self._extract_text(resp.json())
                    results.append({"agent": name, "response": text or "No response"})
                except Exception as e:
                    logger.warning(f"Delegation to {name} failed: {e}")
                    results.append({"agent": name, "response": f"Error: {e}"})

        return results

    def _extract_text(self, resp: dict) -> str | None:
        result = resp.get("result")
        if not result:
            return None
        for artifact in result.get("artifacts", []):
            for part in artifact.get("parts", []):
                if t := part.get("text"):
                    return t
        for part in result.get("message", {}).get("parts", []):
            if t := part.get("text"):
                return t
        return None

    async def _synthesize(self, query: str, results: list[dict]) -> AsyncIterator[str]:
        if not results:
            messages = [
                {"role": "system", "content": "You are a helpful assistant. Answer concisely."},
                {"role": "user", "content": query},
            ]
        else:
            context = "\n\n".join(f"[{r['agent']}]: {r['response']}" for r in results)
            messages = [
                {"role": "system", "content": "Synthesize the agent responses below into a clear answer for the user."},
                {"role": "user", "content": f"Question: {query}\n\nAgent responses:\n{context}"},
            ]

        stream = await self.llm.chat.completions.create(
            model=self.model, messages=messages, stream=True,
            stream_options={"include_usage": True},
        )
        async for chunk in stream:
            delta = chunk.choices[0].delta if chunk.choices else None
            if delta and delta.content:
                yield delta.content

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
    model = os.environ.get("MODEL", "deepseek-v4-flash")

    agent_card = AgentCard(
        name="Assistant",
        description="Routes queries to specialized agents via A2A discovery and synthesizes their responses.",
        supported_interfaces=[
            AgentInterface(protocol_binding="JSONRPC", url=f"http://{host}:{port}/"),
        ],
        version="1.0.0",
        default_input_modes=["text/plain"],
        default_output_modes=["text/plain"],
        capabilities=AgentCapabilities(streaming=True),
        skills=[
            AgentSkill(
                id="orchestrate",
                name="Multi-Agent Orchestration",
                description="Discovers agents, delegates tasks via A2A, synthesizes results",
                tags=["orchestration", "multi-agent", "a2a"],
                examples=["Convert 100 USD to EUR", "Echo hello world", "What is the capital of France?"],
            )
        ],
    )

    handler = DefaultRequestHandler(
        agent_executor=AssistantExecutor(llm, model),
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
