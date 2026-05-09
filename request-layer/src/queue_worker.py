import asyncio
import json
import os
import time
import httpx
import redis.asyncio as redis
from . import cache, stats

KONG_URL = os.getenv("KONG_URL", "http://kong-gateway:8000")

async def queue_worker(redis_client: redis.Redis, http_client: httpx.AsyncClient):
    print("ARIA: Queue worker started")
    while True:
        try:
            agent_names = set()
            async for key in redis_client.scan_iter("queue:*"):
                k = key.decode() if isinstance(key, bytes) else key
                parts = k.split(":", 1)
                if len(parts) == 2:
                    agent_names.add(parts[1])

            for agent_name in agent_names:
                queue_key = f"queue:{agent_name}"
                queue_len = await redis_client.llen(queue_key)
                stats.stats["per_agent_queue_len"][agent_name] = queue_len
                if queue_len == 0:
                    continue

                current_time = int(time.time())
                rate_key = f"rl:{agent_name}:{current_time}"
                current_rate = await redis_client.get(rate_key)
                current_rate = int(current_rate) if current_rate else 0
                current_limit = stats.stats["per_agent_rate_limit"][agent_name]

                if current_rate >= current_limit:
                    continue

                raw = await redis_client.rpop(queue_key)
                if not raw:
                    continue

                queue_len = await redis_client.llen(queue_key)
                stats.stats["per_agent_queue_len"][agent_name] = queue_len

                try:
                    request_info = json.loads(raw)
                    path = request_info.get("path", "")
                    body = request_info.get("body", {})
                    query = ""
                    if isinstance(body, dict):
                        query = body.get("query", "") or body.get("message", "") or ""

                    agent_url = f"{KONG_URL}/agents/{agent_name}/{path}".rstrip("/")
                    response = await http_client.post(
                        agent_url, json=body,
                        headers={"Content-Type": "application/json"},
                        timeout=60.0,
                    )
                    response.raise_for_status()
                    response_json = response.json()

                    request_body_bytes = json.dumps(body).encode()
                    await cache.set_cache(
                        redis_client, agent_name,
                        request_body_bytes, query, response_json,
                    )
                    stats.stats["per_agent_requests"][agent_name] += 1
                    stats.record_request(agent_name)
                    print(f"ARIA Worker: Processed queued request for {agent_name}")

                except httpx.HTTPStatusError as e:
                    print(f"ARIA Worker: Agent error: {e.response.status_code}")
                except Exception as e:
                    print(f"ARIA Worker: Error for {agent_name}: {e}")

        except redis.ConnectionError:
            print("ARIA Worker: Redis connection lost, retrying...")
            await asyncio.sleep(2)
            continue
        except Exception as e:
            print(f"ARIA Worker: Unexpected error: {e}")

        await asyncio.sleep(0.5)
