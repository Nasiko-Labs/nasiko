import os
import asyncio
import random
import time
from fastapi import FastAPI
from pydantic import BaseModel
from typing import Any
from openai import AsyncOpenAI

app = FastAPI(title="Agent B — Medium Agent")
client = AsyncOpenAI(api_key=os.environ.get("OPENAI_API_KEY", "dummy"))

class InvokeRequest(BaseModel):
    query: str = ""
    data: Any = None

@app.post("/invoke")
async def invoke(body: InvokeRequest):
    """Medium agent: uses GPT-4o-mini for analytical responses."""
    try:
        response = await client.chat.completions.create(
            model="gpt-4o-mini",
            messages=[
                {"role": "system", "content": "You are Agent B, an analytical assistant. Provide a structured, bulleted response (max 3 bullets)."},
                {"role": "user", "content": body.query or "Analyze this."}
            ],
            max_tokens=150
        )
        answer = response.choices[0].message.content
        confidence = round(random.uniform(0.75, 0.99), 3)
    except Exception as e:
        answer = f"Error calling OpenAI: {str(e)}"
        confidence = 0.0
        
    return {
        "agent": "agent-b",
        "result": answer,
        "confidence": confidence,
        "timestamp": time.time(),
    }

@app.get("/health")
async def health():
    return {"status": "ok", "agent": "agent-b"}
