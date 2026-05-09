#!/bin/bash
# Nasiko Demo — Load Generator
# Opens the live dashboard, then generates cache + rate-limit traffic.
# The dashboard at http://localhost:8081/dashboard/ tells the story.

set -euo pipefail

ROUTER_URL="${ROUTER_URL:-http://localhost:9100}"
MONITOR_URL="${MONITOR_URL:-http://localhost:8081}"
TOKEN="${1:-}"

if [[ -z "$TOKEN" ]]; then
  echo "Usage: bash run_demo.sh <JWT_TOKEN>"
  echo "       or set TOKEN env var"
  exit 1
fi

echo ""
echo "╔══════════════════════════════════════════════════════╗"
echo "║        Nasiko Router — Live Demo                     ║"
echo "║  Dashboard: ${MONITOR_URL}/dashboard/       ║"
echo "╚══════════════════════════════════════════════════════╝"
echo ""

# ------------------------------------------------------------------
# 1. Cache demonstration — same query 3x
# ------------------------------------------------------------------
echo "[1/3] Cache demonstration (same query ×3)…"
QUERY="Analyze revenue trend for Q3 2025"
for i in 1 2 3; do
  RESULT=$(curl -s -X POST "${ROUTER_URL}/router" \
    -H "Authorization: Bearer ${TOKEN}" \
    -F "session_id=demo-cache" \
    -F "query=${QUERY}" 2>/dev/null | tail -1)
  MSG=$(echo "$RESULT" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print(d.get('message','')[:80])" 2>/dev/null || echo "(parse error)")
  echo "  Call ${i}: ${MSG}"
done
echo "  → First call: cache MISS (~1-2s). Calls 2&3: cache HIT (<10ms)."
echo "  → Check dashboard event stream for cache_hit / cache_miss events."
echo ""

# ------------------------------------------------------------------
# 2. Rate limit burst — 80 concurrent requests
# ------------------------------------------------------------------
echo "[2/3] Rate limit burst (80 concurrent requests)…"
for i in $(seq 1 80); do
  curl -s -X POST "${ROUTER_URL}/router" \
    -H "Authorization: Bearer ${TOKEN}" \
    -F "session_id=burst-${i}" \
    -F "query=Query variant $((i % 5))" \
    > /dev/null 2>&1 &
done
wait
echo "  Done. Check dashboard for queued / rate_limited events."
echo ""

# ------------------------------------------------------------------
# 3. Impact summary
# ------------------------------------------------------------------
echo "[3/3] Impact summary:"
python3 - <<'PYEOF'
import urllib.request, json, sys
try:
    with urllib.request.urlopen("http://localhost:8081/monitoring/impact", timeout=5) as r:
        d = json.load(r)
    print(f"  Cache coverage:       {d.get('cache_coverage_percent', 0):.1f}%")
    print(f"  LLM calls saved:      {d.get('llm_calls_saved', 0)}")
    print(f"  Compute saved (est):  {d.get('compute_saved_estimate_ms', 0):,.0f} ms")
    print(f"  Avg agent latency:    {d.get('avg_latency_uncached_ms', 0):.0f} ms")
    print(f"  Avg cache latency:    {d.get('avg_latency_cached_ms', 8):.0f} ms")
    print(f"  Total requests:       {d.get('total_requests', 0)}")
except Exception as e:
    print(f"  (Could not fetch impact: {e})")
PYEOF
echo ""
echo "Dashboard: ${MONITOR_URL}/dashboard/"
echo "Events API: ${MONITOR_URL}/monitoring/events"
echo ""
