import logging
import os

from dotenv import load_dotenv

load_dotenv()
logging.basicConfig(level=logging.INFO)

# Instrumentation must initialize before a2a-sdk (and anything it imports,
# e.g. Starlette) is imported below: OTel's Starlette instrumentor patches by
# rebinding `starlette.applications.Starlette` to an instrumented subclass, so
# any module that already did `from starlette.applications import Starlette`
# keeps its original, un-instrumented reference forever — no incoming
# traceparent gets extracted, and every request starts an orphan root trace
# instead of joining the platform's session trace. Copied verbatim from
# agents/claude-sdk/src/__main__.py — it is load-bearing there and here.
from telemetry import init_telemetry

init_telemetry()

import click
import uvicorn
from a2a.server.apps import A2AStarletteApplication
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import AgentCapabilities, AgentCard, AgentSkill
from starlette.middleware.cors import CORSMiddleware
from starlette.requests import Request
from starlette.responses import JSONResponse

from agent import CopilotClient, CopilotError
from agent_executor import OSSContributionCopilotExecutor

logger = logging.getLogger(__name__)

client = CopilotClient()


def _skill(id_: str, name: str, description: str, tags: list[str], examples: list[str]) -> AgentSkill:
    return AgentSkill(
        id=id_,
        name=name,
        description=description,
        tags=tags,
        examples=examples,
    )


def _skills() -> list[AgentSkill]:
    """The four skills — field-for-field the same as AgentCard.json (§2.3)."""
    return [
        _skill(
            "contribute",
            "Start Contribution",
            "Start a contribution against a named GitHub repository, subject to "
            "the copilot's writable-repo allow-list.",
            ["github", "contribution", "open-source"],
            ["contribute to Nasiko-Labs/nasiko", "start a contribution against octocat/hello-world"],
        ),
        _skill(
            "contribution_status",
            "Contribution Status",
            "Report the current phase of a contribution thread and any gate "
            "pending on it.",
            ["github", "status"],
            ["status of t_0d35861c6ea8", "what is thread t_0d35861c6ea8 doing"],
        ),
        _skill(
            "list_approvals",
            "List Approvals",
            "List every open human-approval gate across all contribution threads.",
            ["github", "approvals", "gates"],
            ["list open approvals", "what gates are pending"],
        ),
        _skill(
            "decide_approval",
            "Decide Approval",
            "Record a human decision on a named gate. This skill only forwards "
            "a human's decision — it never decides on its own.",
            ["github", "approvals", "gates", "human-in-the-loop"],
            ["approve gate claim on t_0d35861c6ea8", "reject the plan gate on t_0d35861c6ea8"],
        ),
    ]


# -- the REST façade (architecture.md §2.6) — for DronaHQ's tool builder ----
# Same code path as the A2A executor: every handler here calls straight into
# `agent.CopilotClient`, so there is exactly one implementation per
# capability, not two. Mounted at both the bare path (as the architecture doc
# names it) and under `/a2a/` (as the hackathon test suite probes it).


async def _error_response(error: CopilotError) -> JSONResponse:
    return JSONResponse({"detail": error.detail}, status_code=error.status_code)


async def health(request: Request) -> JSONResponse:
    return JSONResponse({"status": "ok"})


async def contribute_route(request: Request) -> JSONResponse:
    body = await request.json()
    try:
        result = await client.start_contribution(str(body["repo"]))
    except CopilotError as error:
        return await _error_response(error)
    except KeyError:
        return JSONResponse({"detail": "missing required field: repo"}, status_code=422)
    return JSONResponse(result)


async def status_route(request: Request) -> JSONResponse:
    thread_id = request.query_params.get("thread_id", "")
    if not thread_id:
        return JSONResponse({"detail": "missing required query param: thread_id"}, status_code=422)
    try:
        result = await client.get_status(thread_id)
    except CopilotError as error:
        return await _error_response(error)
    return JSONResponse(result)


async def approvals_route(request: Request) -> JSONResponse:
    """The projection list (§2.5) — a bare JSON array, one entry per open gate."""
    try:
        rows = await client.list_approvals()
    except CopilotError as error:
        return await _error_response(error)
    return JSONResponse(rows)


async def approve_route(request: Request) -> JSONResponse:
    body = await request.json()
    try:
        result = await client.decide_approval(
            thread_id=str(body["thread_id"]),
            gate=str(body["gate_id"]),
            decision=str(body["decision"]),
            note=str(body.get("note", "")),
        )
    except CopilotError as error:
        return await _error_response(error)
    except KeyError as missing:
        return JSONResponse({"detail": f"missing required field: {missing}"}, status_code=422)
    return JSONResponse(result)


@click.command()
@click.option("--host", default="0.0.0.0")
@click.option("--port", default=10010)
def main(host, port):
    """Starts the OSS Contribution Copilot A2A agent."""
    capabilities = AgentCapabilities(streaming=True)
    agent_url = os.getenv("HOST_OVERRIDE", f"http://{host}:{port}/")
    agent_card = AgentCard(
        name="oss-contribution-copilot",
        description=(
            "Finds, claims, plans, implements and ships open-source "
            "contributions under human approval gates."
        ),
        url=agent_url,
        version="1.0.0",
        default_input_modes=["text/plain"],
        default_output_modes=["text/plain"],
        capabilities=capabilities,
        skills=_skills(),
    )
    request_handler = DefaultRequestHandler(
        agent_executor=OSSContributionCopilotExecutor(),
        task_store=InMemoryTaskStore(),
    )
    server = A2AStarletteApplication(agent_card=agent_card, http_handler=request_handler)
    app = server.build()
    app.add_middleware(
        CORSMiddleware,
        allow_origins=["*"],
        allow_credentials=True,
        allow_methods=["*"],
        allow_headers=["*"],
    )
    for prefix in ("", "/a2a"):
        app.add_route(f"{prefix}/contribute", contribute_route, methods=["POST"])
        app.add_route(f"{prefix}/status", status_route, methods=["GET"])
        app.add_route(f"{prefix}/approvals", approvals_route, methods=["GET"])
        app.add_route(f"{prefix}/approve", approve_route, methods=["POST"])
        app.add_route(f"{prefix}/health", health, methods=["GET"])
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()
