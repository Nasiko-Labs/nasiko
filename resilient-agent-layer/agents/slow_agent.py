import os
import asyncio
import random
import time
from fastapi import FastAPI, HTTPException
from pydantic import BaseModel
from typing import Any
from openai import AsyncOpenAI

app = FastAPI(title="Agent Slow — Heavy/Slow Agent")
client = AsyncOpenAI(api_key=os.environ.get("OPENAI_API_KEY", "dummy"))

_request_count = 0

class InvokeRequest(BaseModel):
    query: str = ""
    data: Any = None

@app.post("/invoke")
async def invoke(body: InvokeRequest):
    """
    Slow agent: uses GPT-4o-mini for deep analysis.
    Simulates an overloaded LLM-style agent.
    """
    global _request_count
    _request_count += 1

    # Simulate occasional errors under load
    if _request_count % 20 == 0:
        raise HTTPException(status_code=500, detail="Agent overloaded")

    try:
        response = await client.chat.completions.create(
            model="gpt-4o-mini",
            messages=[
                {"role": "system", "content": "You are Agent Slow, a highly detailed and verbose expert. Write a comprehensive paragraph answering the user."},
                {"role": "user", "content": body.query or "Tell me a detailed story."}
            ],
            max_tokens=300
        )
        answer = response.choices[0].message.content
        processing_steps = random.randint(5, 15)
    except Exception as e:
        answer = f"Error calling OpenAI: {str(e)}"
        processing_steps = 0
        
    return {
        "agent": "agent-slow",
        "result": answer,
        "processing_steps": processing_steps,
        "timestamp": time.time(),
    }

@app.get("/health")
async def health():
    return {"status": "ok", "agent": "agent-slow", "total_requests": _request_count}
