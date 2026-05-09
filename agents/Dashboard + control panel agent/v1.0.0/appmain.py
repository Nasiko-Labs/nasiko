import os
from pathlib import Path

import httpx
from fastapi import FastAPI
from fastapi.responses import HTMLResponse, JSONResponse
from pydantic import BaseModel

GUARD_URL = os.getenv(
    "SENTINEL_GUARD_URL",
    "http://localhost:9100/agents/sentinel-guard",
)

app = FastAPI(title="SENTINEL Monitor", version="1.0.0")
DASHBOARD_PATH = Path(__file__).parent / "dashboard.html"


# ── Health ─────────────────────────────────────────────────────────
@app.get("/health")
async def health():
    return {"status": "healthy", "agent": "sentinel-monitor", "version": "1.0.0"}


# ── Dashboard ──────────────────────────────────────────────────────
@app.get("/dashboard", response_class=HTMLResponse)
async def dashboard():
    return HTMLResponse(content=DASHBOARD_PATH.read_text())


# ── Stats passthrough ─────────────────────────────────────────────
@app.get("/stats")
async def stats():
    return await _guard_get("/stats")


@app.get("/stats/{agent}")
async def stats_agent(agent: str):
    all_stats = await _guard_get("/stats")
    agents = all_stats.get("agents", {})
    if agent not in agents:
        return JSONResponse(
            status_code=404,
            content={"error": f"No stats for '{agent}'"},
        )
    return {"agent": agent, **agents[agent]}


@app.get("/decisions")
async def decisions():
    return await _guard_get("/decisions")


# ── Control endpoints (proxy to guard) ────────────────────────────
class FlushRequest(BaseModel):
    agent_name: str


class LimitRequest(BaseModel):
    agent_name: str
    requests_per_minute: int


class ThresholdRequest(BaseModel):
    agent_name: str
    threshold: float


@app.post("/cache/flush")
async def cache_flush(req: FlushRequest):
    return await _guard_post("/cache/flush", req.dict())


@app.post("/cache/flush/all")
async def cache_flush_all():
    return await _guard_post("/cache/flush/all", {})


@app.post("/limits/set")
async def limits_set(req: LimitRequest):
    return await _guard_post("/limits/set", req.dict())


@app.post("/cache/threshold")
async def cache_threshold(req: ThresholdRequest):
    return await _guard_post("/cache/threshold", req.dict())


# ── Guard connectivity check ──────────────────────────────────────
@app.get("/guard/status")
async def guard_status():
    try:
        async with httpx.AsyncClient(timeout=5.0) as client:
            r = await client.get(f"{GUARD_URL}/health")
            return {"guard_reachable": True, "guard_health": r.json()}
    except Exception as e:
        return JSONResponse(
            status_code=503,
            content={"guard_reachable": False, "error": str(e)},
        )


# ── Internal helpers ──────────────────────────────────────────────
async def _guard_get(path: str) -> dict:
    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            r = await client.get(f"{GUARD_URL}{path}")
            r.raise_for_status()
            return r.json()
    except Exception as e:
        return {"error": str(e), "tip": "Is sentinel-guard Active?"}


async def _guard_post(path: str, body: dict) -> dict:
    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            r = await client.post(f"{GUARD_URL}{path}", json=body)
            r.raise_for_status()
            return r.json()
    except Exception as e:
        return {"error": str(e)}
