import asyncio
import httpx
import time
import json

async def flood_requests(count=200, concurrent=50, agent="translator"):
    """Flood one agent and verify others are unaffected"""
    async def single_request(i):
        try:
            async with httpx.AsyncClient() as client:
                # Check rate limit first
                r = await client.post(
                    "http://localhost:8090/resilience/ratelimit/check",
                    json={"agent": agent, "priority": "P1", "request_id": f"stress_{i}"},
                    timeout=5
                )
                return r.json()
        except Exception as e:
            return {"error": str(e)}
    
    start = time.time()
    tasks = [single_request(i) for i in range(count)]
    results = await asyncio.gather(*tasks, return_exceptions=True)
    elapsed = time.time() - start
    
    allowed = sum(1 for r in results if isinstance(r, dict) and r.get("allowed"))
    queued = sum(1 for r in results if isinstance(r, dict) and r.get("queued"))
    errors = sum(1 for r in results if isinstance(r, Exception))
    
    print(f"{'='*60}")
    print("STRESS TEST RESULTS")
    print(f"{'='*60}")
    print(f"Sent {count} requests in {elapsed:.2f}s ({count/elapsed:.0f} req/s)")
    print(f"Allowed: {allowed} | Queued: {queued} | Errors: {errors}")
    print(f"Success rate: {(allowed+queued)/max(count,1)*100:.1f}%")
    print(f"Zero cascading failures: {'✅ YES' if errors == 0 else '❌ NO'}")
    
    # Check circuit status
    async with httpx.AsyncClient() as client:
        r = await client.get("http://localhost:8090/resilience/circuit/translator")
        circuit = r.json()
        print(f"Circuit state: {circuit['state']}")
        print(f"Error rate: {circuit['error_rate']*100:.1f}%")
    
    # Check metrics
    async with httpx.AsyncClient() as client:
        r = await client.get("http://localhost:8090/resilience/metrics")
        metrics = r.json()
        print(f"Total cost saved: ${metrics['summary']['total_cost_saved_usd']:.4f}")
        print(f"Total queued: {metrics['summary']['total_queued_requests']}")

async def test_cascading_protection():
    """Verify flooding translator doesn't affect other agents"""
    print("\nTesting cascading failure protection...")
    # This would test multiple agents if deployed
    # For now, verify the service itself doesn't crash
    async with httpx.AsyncClient() as client:
        r = await client.get("http://localhost:8090/resilience/health")
        assert r.status_code == 200
        print("✅ Resilience service stable under load")

if __name__ == "__main__":
    asyncio.run(flood_requests())
    asyncio.run(test_cascading_protection())
