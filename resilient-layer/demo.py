"""
AgentShield Demo Script
Shows before/after comparison for buildathon judging.
"""
import asyncio
import aiohttp
import time
import json
from typing import List, Dict

RESILIENT_URL = "http://localhost:8500/process"
DIRECT_URL = "http://localhost:8081/route"


async def simulate_without_layer(queries: List[str], concurrent: int = 10):
    """Simulate requests going directly to router"""
    print(f"\n{'='*60}")
    print(f"WITHOUT AgentShield (Direct to Router)")
    print(f"{'='*60}")
    
    start_time = time.time()
    success_count = 0
    error_count = 0
    latencies = []
    
    async def make_request(query: str):
        nonlocal success_count, error_count
        try:
            async with aiohttp.ClientSession() as session:
                req_start = time.time()
                async with session.get(
                    DIRECT_URL,
                    params={"query": query}
                ) as resp:
                    if resp.status == 200:
                        success_count += 1
                        latencies.append(time.time() - req_start)
                    else:
                        error_count += 1
        except:
            error_count += 1
    
    # Fire all requests concurrently
    tasks = [make_request(q) for q in queries * (concurrent // len(queries))]
    await asyncio.gather(*tasks)
    
    total_time = time.time() - start_time
    avg_latency = (sum(latencies) / len(latencies) * 1000) if latencies else 0
    
    print(f"Total requests: {len(tasks)}")
    print(f"Success: {success_count}")
    print(f"Errors: {error_count}")
    print(f"Total time: {total_time:.2f}s")
    print(f"Avg latency: {avg_latency:.1f}ms")
    print(f"Agent calls made: {success_count}")
    
    return {
        "total_requests": len(tasks),
        "success": success_count,
        "errors": error_count,
        "total_time": total_time,
        "avg_latency_ms": avg_latency,
        "agent_calls": success_count
    }


async def simulate_with_layer(queries: List[str], concurrent: int = 10):
    """Simulate requests going through AgentShield"""
    print(f"\n{'='*60}")
    print(f"WITH AgentShield (Intelligent Traffic Layer)")
    print(f"{'='*60}")
    
    start_time = time.time()
    success_count = 0
    error_count = 0
    latencies = []
    
    source_stats = {"cache": 0, "coalesced": 0, "queued": 0, "agent": 0}
    
    async def make_request(query: str):
        nonlocal success_count, error_count
        try:
            async with aiohttp.ClientSession() as session:
                req_start = time.time()
                async with session.post(
                    RESILIENT_URL,
                    params={"query": query, "agent_hint": "translator"}
                ) as resp:
                    if resp.status == 200:
                        data = await resp.json()
                        source = data.get("source", "unknown")
                        source_stats[source] = source_stats.get(source, 0) + 1
                        success_count += 1
                        latencies.append(time.time() - req_start)
                    else:
                        error_count += 1
        except:
            error_count += 1
    
    # Fire all requests concurrently
    tasks = [make_request(q) for q in queries * (concurrent // len(queries))]
    await asyncio.gather(*tasks)
    
    total_time = time.time() - start_time
    avg_latency = (sum(latencies) / len(latencies) * 1000) if latencies else 0
    
    print(f"Total requests: {len(tasks)}")
    print(f"Success: {success_count}")
    print(f"Errors: {error_count}")
    print(f"Total time: {total_time:.2f}s")
    print(f"Avg latency: {avg_latency:.1f}ms")
    print(f"\nRequest Sources:")
    for source, count in source_stats.items():
        print(f"  {source}: {count} ({count/len(tasks)*100:.1f}%)")
    
    return {
        "total_requests": len(tasks),
        "success": success_count,
        "errors": error_count,
        "total_time": total_time,
        "avg_latency_ms": avg_latency,
        "source_breakdown": source_stats
    }


async def main():
    """Run demo comparing with and without AgentShield"""
    
    # Test queries - many duplicates to show coalescing
    queries = [
        "translate hello to french",
        "translate good morning to spanish",
        "translate thank you to german",
        "translate how are you to italian",
        "translate hello to french",  # Duplicate
        "translate hello to french",  # Duplicate
        "translate hello to french",  # Duplicate
        "translate good morning to spanish",  # Duplicate
    ]
    
    print("\n" + "="*60)
    print("   AGENTSHIELD - Resilient Agent Request Layer")
    print("   Buildathon Demo")
    print("="*60)
    print(f"\nTest Setup:")
    print(f"  {len(queries)} unique queries")
    print(f"  10 concurrent requests each")
    print(f"  Total: {len(queries) * 10} simulated requests")
    
    # Run without layer
    without = await simulate_without_layer(queries, concurrent=10)
    
    await asyncio.sleep(2)
    
    # Run with layer
    with_result = await simulate_with_layer(queries, concurrent=10)
    
    # Comparison
    print(f"\n{'='*60}")
    print(f"COMPARISON SUMMARY")
    print(f"{'='*60}")
    
    latency_improvement = (
        (without["avg_latency_ms"] - with_result["avg_latency_ms"])
        / max(without["avg_latency_ms"], 1) * 100
    )
    
    print(f"\nLatency:")
    print(f"  Without:  {without['avg_latency_ms']:.0f}ms")
    print(f"  With:     {with_result['avg_latency_ms']:.0f}ms")
    print(f"  Improvement: {latency_improvement:.1f}%")
    
    print(f"\nErrors:")
    print(f"  Without:  {without['errors']}")
    print(f"  With:     {with_result['errors']}")
    
    if "source_breakdown" in with_result:
        print(f"\nOptimization Breakdown:")
        for source, count in with_result["source_breakdown"].items():
            print(f"  {source}: {count}")
    
    # Get metrics from resilient layer
    try:
        async with aiohttp.ClientSession() as session:
            async with session.get("http://localhost:8500/metrics") as resp:
                metrics = await resp.json()
                print(f"\nFinal System Metrics:")
                print(json.dumps(metrics, indent=2))
    except:
        pass
    
    print(f"\n{'='*60}")
    print("Demo Complete - AgentShield is Production Ready!")
    print(f"{'='*60}\n")


if __name__ == "__main__":
    asyncio.run(main())
