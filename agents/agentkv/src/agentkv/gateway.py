"""Private authenticated ingress to one engine, including atomic lease hints."""
from contextlib import asynccontextmanager
from collections import OrderedDict
import hashlib
import hmac
import json
import os
from pathlib import Path
import time
import tempfile

import httpx
from fastapi import FastAPI, Header, HTTPException, Request
from fastapi.responses import Response
from pydantic import BaseModel, ConfigDict, Field

DIRECTORY = Path(os.getenv("AGENTKV_ENGINE_DIR", "/tmp/agentkv-engine"))
TOKEN = os.environ["AGENTKV_PROVIDER_KEY"]
REQUEST_USAGE = OrderedDict()
NAMESPACE = hashlib.sha256(("agentkv-owner:" + TOKEN).encode()).hexdigest()


@asynccontextmanager
async def lifespan(app):
    app.state.upstream = httpx.AsyncClient(base_url="http://127.0.0.1:8000", timeout=180)
    yield
    await app.state.upstream.aclose()


app = FastAPI(lifespan=lifespan, docs_url=None, redoc_url=None)


def authenticate(value):
    if not value or not hmac.compare_digest(value.encode(), ("Bearer " + TOKEN).encode()):
        raise HTTPException(401, "Authentication required")


class Hint(BaseModel):
    model_config = ConfigDict(extra="forbid")
    request_id: str = Field(min_length=1, max_length=256)
    score: float = Field(gt=0, allow_inf_nan=False)
    lease_seconds: float = Field(gt=0, le=30)


class Policy(BaseModel):
    model_config = ConfigDict(extra="forbid")
    enabled: bool
    hints: list[Hint] = Field(default_factory=list, max_length=3)


@app.get("/health")
async def health(authorization: str | None = Header(default=None)):
    authenticate(authorization)
    result = await app.state.upstream.get("/health")
    return {"ready": result.status_code == 200, "gpu_started_at": float(os.getenv("AGENTKV_GPU_STARTED_AT", "0"))}


@app.post("/cache-hints")
def cache_hints(policy: Policy, authorization: str | None = Header(default=None)):
    authenticate(authorization)
    DIRECTORY.mkdir(mode=0o700, exist_ok=True)
    now = time.time()
    content = {"enabled": policy.enabled, "hints": [
        {"request_id": h.request_id, "score": h.score, "expires_at": now + h.lease_seconds}
        for h in policy.hints]}
    with tempfile.NamedTemporaryFile(mode="w", dir=DIRECTORY, delete=False) as stream:
        stream.write(json.dumps(content))
        path = Path(stream.name)
    path.replace(DIRECTORY / "hints.json")
    return {"status": "proposed", "timestamp": now}


@app.get("/engine-state")
def state(authorization: str | None = Header(default=None)):
    authenticate(authorization)
    try:
        return json.loads((DIRECTORY / "state.json").read_text())
    except (OSError, ValueError):
        return {"adapter": "starting", "groups": [], "acknowledgements": []}


@app.get("/engine-metrics")
async def metrics(authorization: str | None = Header(default=None)):
    authenticate(authorization)
    result = await app.state.upstream.get("/metrics")
    return Response(result.content, status_code=result.status_code, media_type="text/plain")


@app.post("/reset-cache")
async def reset(authorization: str | None = Header(default=None)):
    authenticate(authorization)
    result = await app.state.upstream.post("/reset_prefix_cache")
    return Response(result.content, status_code=result.status_code)


@app.get('/request-usage/{request_id}')
def request_usage(request_id: str, authorization: str | None = Header(default=None)):
    authenticate(authorization)
    if request_id not in REQUEST_USAGE:
        raise HTTPException(404,'Request telemetry expired or unavailable')
    return REQUEST_USAGE[request_id]


@app.post("/v1/chat/completions")
async def completions(request: Request, authorization: str | None = Header(default=None)):
    authenticate(authorization)
    payload = await request.json()
    if payload.get("stream"):
        raise HTTPException(400, "This bounded demo uses non-streaming requests")
    payload["cache_salt"] = NAMESPACE
    started=time.perf_counter()
    result = await app.state.upstream.post("/v1/chat/completions", json=payload)
    if result.status_code==200:
        answer=result.json()
        entry={'usage':answer.get('usage'),'request_id':answer.get('id'),
               'gateway_seconds':time.perf_counter()-started,'source':'vllm_response'}
        for key in (answer.get('id'), payload.get('request_id')):
            if isinstance(key, str) and key:
                REQUEST_USAGE[key]=entry
        while len(REQUEST_USAGE)>4096: REQUEST_USAGE.popitem(last=False)
    return Response(result.content, status_code=result.status_code, media_type="application/json")
