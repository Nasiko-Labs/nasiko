import os
import time
import httpx
import uvicorn
from fastapi import FastAPI, Request
from redisvl.extensions.llmcache import SemanticCache
from redisvl.utils.vectorize import OpenAITextVectorizer
from redis.exceptions import ConnectionError

app = FastAPI(title="Nasiko Resilient Layer")

# --- STEP 1: RESILIENT INITIALIZATION ---
# We wait for Redis to be ready so the container doesn't crash on start
cache = None
while cache is None:
    try:
        vectorizer = OpenAITextVectorizer(
            model="text-embedding-3-small",
            api_config={"api_key": os.getenv("OPENAI_API_KEY")}
        )
        cache = SemanticCache(
            name="nasiko_cache",
            redis_url="redis://redis:6379",
            vectorizer=vectorizer,
            distance_threshold=0.1
        )
        print("✅ Successfully connected to Redis Stack!")
    except Exception as e:
        print(f"Waiting for Redis Stack... {e}")
        time.sleep(5)

KONG_URL = "http://kong-gateway:8000"
AGENT_LIMITS = {"a2a-translator": 10}

@app.post("/request/{agent_name}")
async def handle_request(agent_name: str, request: Request):
    data = await request.json()
    prompt = data.get("query")

    # A. Cache Check
    if cached_response := cache.check(prompt=prompt):
        return {"source": "semantic_cache", "data": cached_response[0]["response"]}

    # B. Rate Limit/Queue
    now = int(time.time())
    limit_key = f"limiter:{agent_name}:{now // 60}"
    count = await cache._redis_client.incr(limit_key)
    
    if count > AGENT_LIMITS.get(agent_name, 5):
        await cache._redis_client.lpush(f"queue:{agent_name}", prompt)
        return {"status": "queued", "message": "Agent overloaded."}

    # C. Forward to Agent
    async with httpx.AsyncClient() as client:
        response = await client.post(f"{KONG_URL}/agents/{agent_name}/chat", json=data)
        agent_result = response.json()

    # D. Store in Cache
    cache.store(prompt=prompt, response=str(agent_result))
    return {"source": "agent_fleet", "data": agent_result}

@app.get("/metrics")
async def get_metrics():
    return {"status": "online", "cache_hits": "active"}

if __name__ == "__main__":
    uvicorn.run(app, host="0.0.0.0", port=8090)