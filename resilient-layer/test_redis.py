import asyncio
import redis.asyncio as redis

async def main():
    r = redis.Redis(host="localhost", port=6379, decode_responses=True)
    await r.set("test", "1")
    cursor = 0
    cursor, batch = await r.scan(cursor, match="*")
    print(repr(cursor))

asyncio.run(main())
