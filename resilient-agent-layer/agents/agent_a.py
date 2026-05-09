import os
import asyncio
import random
import time
from fastapi import FastAPI
from pydantic import BaseModel
from typing import Any
from openai import AsyncOpenAI

app = FastAPI(title="Agent A — Fast Agent")
client = AsyncOpenAI(api_key=os.environ.get("OPENAI_API_KEY", "dummy"))

class InvokeRequest(BaseModel):
    query: str = ""
    data: Any = None

@app.post("/invoke")
async def invoke(body: InvokeRequest):
    """Fast agent: uses GPT-4o-mini to answer concisely."""
    try:
        response = await client.chat.completions.create(
            model="gpt-4o-mini",
            messages=[
                {"role": "system", "content": "You are Agent A, a fast and concise assistant. Answer the user's query in 1-2 sentences maximum."},
                {"role": "user", "content": body.query or "Hello"}
            ],
            max_tokens=50
        )
        answer = response.choices[0].message.content
        tokens = response.usage.total_tokens
    except Exception as e:
        answer = f"Error calling OpenAI: {str(e)}"
        tokens = 0
        
    return {
        "agent": "agent-a",
        "result": answer,
        "tokens_used": tokens,
        "timestamp": time.time(),
    }

@app.get("/health")
async def health():
    return {"status": "ok", "agent": "agent-a"}
