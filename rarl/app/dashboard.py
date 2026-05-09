import asyncio
import json

from fastapi import APIRouter, Request
from fastapi.templating import Jinja2Templates
from sse_starlette.sse import EventSourceResponse

router = APIRouter(tags=["dashboard"])
templates = Jinja2Templates(directory="app/templates")


@router.get("/dashboard")
async def dashboard(request: Request):
    return templates.TemplateResponse("dashboard.html", {"request": request})


@router.get("/admin/stream")
async def stream(request: Request):
    async def event_generator():
        while True:
            if await request.is_disconnected():
                break
            snapshot = request.app.state.metrics.snapshot()
            lanes = request.app.state.lanes
            snapshot["lanes"] = [
                {
                    "agent_id": lane.agent_id,
                    "queue_depth": lane.queue_depth,
                    "rps": lane.bucket.rate,
                    "served": lane.served,
                    "rejected": lane.rejected,
                    "p95_ms": round(lane.p95 * 1000, 2),
                }
                for lane in lanes.values()
            ]
            yield {"event": "snapshot", "data": json.dumps(snapshot)}
            await asyncio.sleep(0.5)

    return EventSourceResponse(event_generator())
