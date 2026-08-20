from fastapi import FastAPI, Request
from pydantic import BaseModel
from openai import OpenAI
from slowapi import Limiter
from slowapi.util import get_remote_address
from slowapi.errors import RateLimitExceeded
from fastapi.responses import JSONResponse
import redis
import os
import time

# FastAPI app
app = FastAPI(
    title="Unified Agent Traffic Controller",
    description="AI Traffic Controller with Redis Cache, Queue, Rate Limiting, Metrics, and OpenAI Integration",
    version="2.0.0"
)

# OpenAI client
client = OpenAI(
    api_key=os.getenv("OPENAI_API_KEY")
)

# Redis client
redis_client = redis.Redis(
    host="localhost",
    port=6379,
    decode_responses=True
)

# Rate limiter
limiter = Limiter(key_func=get_remote_address)
app.state.limiter = limiter

# Queue system
queue = []

# Request model
class RequestModel(BaseModel):
    agent: str
    query: str

# Rate limit handler
@app.exception_handler(RateLimitExceeded)
async def rate_limit_handler(request: Request, exc: RateLimitExceeded):
    return JSONResponse(
        status_code=429,
        content={
            "error": "Too many requests",
            "detail": "Rate limit exceeded"
        }
    )

# Main routing endpoint
@app.post("/route")
@limiter.limit("10/minute")
async def route(request: Request, req: RequestModel):

    key = f"{req.agent}:{req.query}"

    # Redis cache lookup
    cached_response = redis_client.get(key)

    if cached_response:
        return {
            "source": "redis-cache",
            "response": cached_response
        }

    # Queue overload handling
    if len(queue) >= 5:
        queue.append(req.query)

        return {
            "status": "queued",
            "position": len(queue),
            "message": "Server busy, request added to queue"
        }

    # Add request to queue
    queue.append(req.query)

    try:
        # Simulate processing delay
        time.sleep(1)

        # OpenAI response
        completion = client.chat.completions.create(
            model="gpt-3.5-turbo",
            messages=[
                {
                    "role": "system",
                    "content": f"You are a helpful {req.agent} AI agent."
                },
                {
                    "role": "user",
                    "content": req.query
                }
            ]
        )

        response = completion.choices[0].message.content

        # Save response in Redis cache
        redis_client.set(key, response)

        return {
            "source": "agent",
            "response": response
        }

    except Exception as e:
        return {
            "error": str(e)
        }

    finally:
        # Remove completed request from queue
        if req.query in queue:
            queue.remove(req.query)

# Health endpoint
@app.get("/health")
async def health():
    return {
        "status": "healthy"
    }

# Metrics endpoint
@app.get("/metrics")
async def metrics():

    cached_keys = redis_client.keys()

    return {
        "system_status": "healthy",
        "cache_type": "Redis",
        "cache_size": redis_client.dbsize(),
        "queue_size": len(queue),
        "cached_keys": cached_keys,
        "rate_limit": "10 requests/minute",
        "architecture": "Unified Agent Traffic Controller",
        "features": [
            "OpenAI Integration",
            "Redis Distributed Cache",
            "Queue Management",
            "Rate Limiting",
            "Metrics Monitoring",
            "AI Request Orchestration"
        ]
    }

# Root endpoint
@app.get("/")
async def root():
    return {
        "message": "Unified Agent Traffic Controller Running 🚀",
        "version": "2.0.0"
    }