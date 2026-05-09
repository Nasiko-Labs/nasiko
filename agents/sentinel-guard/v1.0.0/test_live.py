"""Test full pipeline with proper a2a JSON-RPC format."""
import httpx
import time
import uuid

KONG = "http://localhost:9100"
SENTINEL = "http://localhost:8500"

def send_translation(text):
    """Send a properly formatted a2a translation request through Kong."""
    msg_id = str(uuid.uuid4())
    payload = {
        "jsonrpc": "2.0",
        "method": "message/send",
        "id": msg_id,
        "params": {
            "message": {
                "role": "user",
                "messageId": msg_id,
                "parts": [{"kind": "text", "text": text}],
            }
        },
    }
    r = httpx.post(
        f"{KONG}/agents/agent-a2a-translator",
        json=payload,
        headers={"Content-Type": "application/json"},
        timeout=60,
    )
    return r

# ── Test 1: First request (should be forwarded to agent) ──
print("=" * 60)
print("  LIVE INTEGRATION TEST")
print("=" * 60)

print("\n[1] Sending: 'Translate hello world to French'")
r1 = send_translation("Translate hello world to French")
print(f"    Status: {r1.status_code}")
if r1.status_code == 200:
    data = r1.json()
    if "result" in data:
        # Extract the agent's response text
        try:
            parts = data["result"]["message"]["parts"]
            for p in parts:
                if p.get("kind") == "text":
                    print(f"    Agent response: {p['text'][:200]}")
                    break
        except (KeyError, TypeError):
            print(f"    Response: {str(data)[:200]}")
    elif "error" in data:
        print(f"    Error: {data['error']['message']}")
    else:
        print(f"    Response: {str(data)[:200]}")

# ── Test 2: Same request again (should be CACHE HIT) ──
print("\n[2] Sending SAME query again: 'Translate hello world to French'")
r2 = send_translation("Translate hello world to French")
print(f"    Status: {r2.status_code}")
if r2.status_code == 200:
    data2 = r2.json()
    print(f"    Response: {str(data2)[:200]}")

# ── Test 3: Similar query (should be SEMANTIC HIT) ──
print("\n[3] Sending SIMILAR query: 'How do you say hello world in French'")
r3 = send_translation("How do you say hello world in French")
print(f"    Status: {r3.status_code}")

# ── Check dashboard stats ──
time.sleep(2)
stats = httpx.get(f"{SENTINEL}/stats", timeout=10).json()
s = stats["summary"]
print("\n" + "=" * 60)
print("  SENTINEL GUARD DASHBOARD STATS")
print("=" * 60)
print(f"  Total requests:    {s['total_requests']}")
print(f"  Cache hits:        {s['total_cache_hits']}")
print(f"  Cache misses:      {s['total_cache_misses']}")
print(f"  Semantic hits:     {s['total_semantic_hits']}")
print(f"  Hit rate:          {s['cache_hit_rate_pct']}%")
print(f"  Forwarded:         {s['total_forwarded']}")
print(f"  Rejected:          {s['total_rejected']}")

print("\n  Per-agent breakdown:")
for ag, data in stats["per_agent"].items():
    print(f"    {ag}: {data['total']} req, {data['hits']} hits, {data['hit_rate_pct']}% hit rate")

print("\n  Recent decisions:")
for d in stats.get("recent_decisions", [])[:5]:
    print(f"    [{d['outcome']}] {d['agent']} - {d.get('query','')[:50]} ({d.get('latency_ms',0):.0f}ms)")

print("\n" + "=" * 60)
print("  NOW open http://localhost:8500/dashboard to see LIVE data!")
print("  Send messages in Nasiko UI and watch the dashboard update!")
print("=" * 60)
