import asyncio
import random
import time
from fastapi import FastAPI
from pydantic import BaseModel
from typing import Any

app = FastAPI(title="Agent A — Fast Agent")

class InvokeRequest(BaseModel):
    query: str = ""
    data: Any = None

@app.post("/invoke")
async def invoke(body: InvokeRequest):
    """Fast agent: responds in 50–150ms."""
    await asyncio.sleep(random.uniform(0.05, 0.15))
    return {
        "agent": "agent-a",
        "result": f"Processed: {body.query}",
        "tokens_used": random.randint(100, 500),
        "timestamp": time.time(),
    }

@app.get("/health")
async def health():
    return {"status": "ok", "agent": "agent-a"}
