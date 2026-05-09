import asyncio
import httpx

async def main():
    async with httpx.AsyncClient() as client:
        # Request 1
        res1 = await client.post("http://localhost:8500/process", data={"query": "hello world", "route": "translator"})
        print("Res 1:", res1.json())
        # Request 2
        res2 = await client.post("http://localhost:8500/process", data={"query": "hello world", "route": "translator"})
        print("Res 2:", res2.json())

asyncio.run(main())
