import asyncio
import random
import time
from fastapi import FastAPI
from pydantic import BaseModel
from typing import Any

app = FastAPI(title="Agent B — Medium Agent")

class InvokeRequest(BaseModel):
    query: str = ""
    data: Any = None

@app.post("/invoke")
async def invoke(body: InvokeRequest):
    """Medium agent: responds in 300–700ms."""
    await asyncio.sleep(random.uniform(0.3, 0.7))
    return {
        "agent": "agent-b",
        "result": f"Analyzed: {body.query}",
        "confidence": round(random.uniform(0.75, 0.99), 3),
        "timestamp": time.time(),
    }

@app.get("/health")
async def health():
    return {"status": "ok", "agent": "agent-b"}
