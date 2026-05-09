import os
from fastapi import FastAPI, Request, HTTPException
from redisvl.extensions.llmcache import SemanticCache
import httpx
import time

app = FastAPI(title="Nasiko Resilient Layer")

# 1. Initialize Semantic Cache (Success Metric: Reduced Duplicate Processing)
# distance_threshold 0.1 means 90% similarity required for a hit
cache = SemanticCache(
    name="nasiko_cache",
    redis_url="redis://localhost:6379",
    distance_threshold=0.1 
)

# 2. Configure Limits (Requirement: Per-agent rate limits)
AGENT_LIMITS = {"translator": 10} # 10 requests per minute
KONG_URL = "http://localhost:9100"

@app.post("/request/{agent_name}")
async def handle_request(agent_name: str, request: Request):
    data = await request.json()
    prompt = data.get("query")

    # --- STEP A: SEMANTIC CACHE CHECK ---
    if cached_response := cache.check(prompt=prompt):
        return {"source": "semantic_cache", "data": cached_response[0]["response"]}

    # --- STEP B: RATE LIMIT & QUEUEING ---
    # (Requirement: Excess traffic should be queued)
    # Check rate limit in Redis (Sliding Window)
    now = int(time.time())
    limit_key = f"limiter:{agent_name}:{now // 60}"
    count = await cache._redis_client.incr(limit_key)
    
    if count > AGENT_LIMITS.get(agent_name, 5):
        # Push to Redis List as a simple queue
        await cache._redis_client.lpush(f"queue:{agent_name}", prompt)
        return {"status": "queued", "message": "Agent overloaded, request pending."}

    # --- STEP C: FORWARD TO AGENT ---
    async with httpx.AsyncClient() as client:
        # Forward to the actual Nasiko Kong endpoint
        response = await client.post(f"{KONG_URL}/agents/{agent_name}/chat", json=data)
        agent_result = response.json()

    # --- STEP D: POPULATE CACHE ---
    cache.store(prompt=prompt, response=str(agent_result))
    
    return {"source": "agent_fleet", "data": agent_result}

# 3. OPERATIONAL CONTROLS (Requirement: Monitoring Endpoints)
@app.get("/metrics")
async def get_metrics():
    return {
        "cache_size": await cache._redis_client.dbsize(),
        "active_queues": {k: await cache._redis_client.llen(f"queue:{k}") for k in AGENT_LIMITS.keys()}
    }

if __name__ == "__main__":
    import uvicorn
    print("🚀 Starting Nasiko Resilient Layer on http://localhost:8090")
    uvicorn.run(app, host="0.0.0.0", port=8090)