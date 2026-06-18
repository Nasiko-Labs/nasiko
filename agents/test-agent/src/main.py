"""
test — Nasiko Agent
Entry point: FastAPI app exposing the A2A interface.
"""

import json
import os
from pathlib import Path

from telemetry import init_telemetry

init_telemetry("test-agent")

from agent import run
from fastapi import FastAPI, HTTPException
from fastapi.responses import JSONResponse
from pydantic import BaseModel

app = FastAPI(title="test", version="0.1.0")

_CARD_PATH = Path(__file__).parent.parent / "AgentCard.json"


class InvokeRequest(BaseModel):
    input: str
    session_id: str = ""


@app.get("/")
async def health():
    return {"status": "ok", "agent": "test"}


@app.get("/.well-known/agent.json")
async def agent_card():
    if not _CARD_PATH.exists():
        raise HTTPException(status_code=404, detail="AgentCard.json not found")
    return JSONResponse(json.loads(_CARD_PATH.read_text()))


@app.post("/invoke")
async def invoke(req: InvokeRequest):
    try:
        result = await run(req.input) if asyncio_run_check() else run(req.input)
        return {"output": result}
    except Exception as exc:
        raise HTTPException(status_code=500, detail=str(exc))


def asyncio_run_check():
    import inspect

    import agent as _agent_mod
    return inspect.iscoroutinefunction(getattr(_agent_mod, "run", None))


if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=int(os.getenv("PORT", "8000")))
