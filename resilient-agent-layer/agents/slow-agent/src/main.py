import asyncio
import random
import time
from fastapi import FastAPI, HTTPException
from pydantic import BaseModel
from typing import Any

app = FastAPI(title="Agent Slow — Heavy/Slow Agent")

_request_count = 0

class InvokeRequest(BaseModel):
    query: str = ""
    data: Any = None

@app.post("/invoke")
async def invoke(body: InvokeRequest):
    """
    Slow agent: responds in 1.5–4s.
    Simulates an overloaded LLM-style agent.
    """
    global _request_count
    _request_count += 1

    # Simulate occasional errors under load
    if _request_count % 20 == 0:
        raise HTTPException(status_code=500, detail="Agent overloaded")

    await asyncio.sleep(random.uniform(1.5, 4.0))
    return {
        "agent": "agent-slow",
        "result": f"Deep analysis complete for: {body.query}",
        "processing_steps": random.randint(5, 15),
        "timestamp": time.time(),
    }

@app.get("/health")
async def health():
    return {"status": "ok", "agent": "agent-slow", "total_requests": _request_count}
