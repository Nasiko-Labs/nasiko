"""Update Kong + test the full flow end-to-end."""
import httpx
import time

# Wait for sentinel-guard
for i in range(10):
    try:
        r = httpx.get("http://localhost:8500/health", timeout=5)
        if r.status_code == 200:
            print("Sentinel Guard is ready")
            break
    except Exception:
        pass
    print(f"Waiting... ({i+1})")
    time.sleep(2)

kong = "http://localhost:9101"

# Update Kong translator service to go through sentinel-guard
r = httpx.patch(
    f"{kong}/services/agent-a2a-translator",
    json={"host": "sentinel-guard", "port": 8000, "path": "/agents/agent-a2a-translator"},
    timeout=10,
)
print(f"Kong update: {r.status_code}")
u = r.json()
print(f"Routing: {u['host']}:{u['port']}{u.get('path','')}")

# Now test by sending a request directly through Kong (same path the UI uses)
print("\n--- Testing: Translate hello world to French ---")
test_r = httpx.post(
    "http://localhost:9100/agents/agent-a2a-translator",
    json={
        "jsonrpc": "2.0",
        "method": "message/send",
        "id": "test-1",
        "params": {
            "message": {
                "role": "user",
                "parts": [{"kind": "text", "text": "Translate hello world to French"}],
            }
        },
    },
    headers={"Content-Type": "application/json"},
    timeout=60,
)
print(f"Response status: {test_r.status_code}")
print(f"Response: {test_r.text[:500]}")

# Check sentinel guard stats
time.sleep(2)
stats = httpx.get("http://localhost:8500/stats", timeout=10).json()
print(f"\nSentinel Guard Stats:")
print(f"  Total requests: {stats['summary']['total_requests']}")
print(f"  Cache hits: {stats['summary']['total_cache_hits']}")
print(f"  Forwarded: {stats['summary']['total_forwarded']}")
print(f"  Per-agent: {stats['per_agent']}")
